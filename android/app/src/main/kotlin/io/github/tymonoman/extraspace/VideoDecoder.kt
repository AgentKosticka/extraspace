package io.github.tymonoman.extraspace

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Surface
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import kotlin.concurrent.thread

/**
 * Hardware H.264 decode straight onto a [Surface].
 *
 * Frames are handed to the codec as they arrive and released as soon as they are
 * decoded. Nothing is queued deliberately: for a live second display a late frame
 * is worthless, so the goal is to keep the codec's input queue as close to empty
 * as possible and report its depth back to the host, which lowers bitrate when it
 * starts to grow.
 */
class VideoDecoder(private val surface: Surface, private val traceLatency: Boolean = false) {
    @Volatile private var codec: MediaCodec? = null
    @Volatile private var running = false
    private var drainThread: Thread? = null

    /** Codec input queue depth -- the host's main signal that we are falling behind. */
    val queueDepth: Int get() = pendingInputs.get()
    val framesDecoded = AtomicLong(0)
    val framesDropped = AtomicLong(0)
    /** Device-clock microseconds reported by the codec for its last surface render. */
    val renderedAtUs = AtomicLong(0)
    val lastFramePtsUs = AtomicLong(0)

    /** Frames submitted but not yet released for display. Touched by two threads. */
    private val pendingInputs = AtomicInteger(0)
    private val inputPace = PaceWatch("decode_in")
    private val outputPace = PaceWatch("decode_out")
    private val submittedPts = ConcurrentHashMap<Long, Long>()

    @Synchronized
    fun start(width: Int, height: Int, csd: ByteArray?) {
        stop()
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height).apply {
            // Tells the decoder to minimise internal buffering. Without this most
            // MediaCodec implementations hold 2-4 frames, which alone can exceed
            // our entire latency budget.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                setInteger(MediaFormat.KEY_ALLOW_FRAME_DROP, 1)
            }
            // The host converts desktop sRGB to full-range BT.709 YUV. The VA
            // encoder writes no VUI, so without these keys MediaCodec assumes
            // limited range (16-235). TextureView is then composited as
            // V0_SRGB / full range, and the picture goes washed-out. Pinning
            // full range here stops the decoder from expanding values that are
            // already 0-255.
            setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709)
            setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_FULL)
            setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO)
            // SPS/PPS, if the host sent them ahead of the first frame. The host
            // also repeats them inline on every keyframe, so this is belt and
            // braces for the very first connection.
            csd?.let { setByteBuffer("csd-0", ByteBuffer.wrap(it)) }
        }

        codec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC).apply {
            configure(format, surface, null, 0)
            setOnFrameRenderedListener({ renderedCodec, localPtsUs, renderedNs ->
                // A queued callback from an old codec must not update a new stream.
                if (codec !== renderedCodec) return@setOnFrameRenderedListener
                val hostPts = submittedPts.remove(localPtsUs)
                if (renderedNs > 0) renderedAtUs.set(renderedNs / 1000)
                if (hostPts != null) lastFramePtsUs.set(hostPts)
                if (traceLatency) {
                    Log.i(TAG, "latency stage=rendered local_pts_us=$localPtsUs render_us=${renderedNs / 1000 - localPtsUs} host_pts_us=$hostPts")
                }
            }, Handler(Looper.getMainLooper()))
            Log.i(TAG, "decoder selected name=$name low_latency_supported=${codecInfo.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC).isFeatureSupported("low-latency")}")
            start()
        }
        framesDecoded.set(0)
        framesDropped.set(0)
        running = true
        // Output is drained on its own thread rather than piggybacking on input.
        // Draining only when a new frame arrives means that when the desktop goes
        // idle -- which, since mutter only sends on damage, is most of the time --
        // the last frame sits decoded but unrendered until something else changes.
        drainThread = thread(name = "xs-decode-drain") { drainLoop() }
        Log.i(TAG, "decoder started ${width}x$height low-latency=${Build.VERSION.SDK_INT >= Build.VERSION_CODES.R}")
    }

    /**
     * Submits one access unit. Returns false if the codec could not accept it,
     * which means we are behind and the frame is discarded.
     */
    @Synchronized
    fun decode(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean): Boolean {
        val mc = codec ?: return false
        return try {
            // Short timeout rather than blocking: if the codec is saturated we
            // would rather drop this frame than stall the socket reader and let
            // even more frames pile up behind it.
            val waitStart = System.nanoTime()
            val index = mc.dequeueInputBuffer(INPUT_TIMEOUT_US)
            if (index < 0) {
                framesDropped.incrementAndGet()
                return false
            }
            val waitMs = (System.nanoTime() - waitStart) / 1_000_000L
            mc.getInputBuffer(index)?.apply {
                clear()
                put(data, 0, length)
            }
            val flags = if (isConfig) MediaCodec.BUFFER_FLAG_CODEC_CONFIG else 0
            // Use a unique timestamp on the device clock to measure decode time.
            // Presentation remains explicit in releaseOutputBuffer; host PTS is
            // from an unrelated clock and must never schedule tablet playback.
            val localPtsUs = waitStart / 1000
            if (!isConfig) {
                // A decoder can drop outputs; bound diagnostics memory too.
                if (submittedPts.size >= 256) submittedPts.clear()
                submittedPts[localPtsUs] = ptsUs
            }
            // Increment before submission: the independent drain thread can
            // release an output before queueInputBuffer returns. Config buffers
            // do not produce a corresponding picture and must not inflate depth.
            if (!isConfig) pendingInputs.incrementAndGet()
            try {
                mc.queueInputBuffer(index, 0, length, localPtsUs, flags)
            } catch (e: IllegalStateException) {
                if (!isConfig) {
                    submittedPts.remove(localPtsUs)
                    pendingInputs.updateAndGet { (it - 1).coerceAtLeast(0) }
                }
                throw e
            }
            inputPace.observe("bytes=$length pts_us=$ptsUs wait_ms=$waitMs")
            true
        } catch (e: IllegalStateException) {
            Log.e(TAG, "decoder rejected input", e)
            false
        }
    }

    /**
     * Releases finished frames to the surface as soon as they are ready.
     *
     * Blocks on the codec rather than spinning, so an idle desktop costs nothing
     * while a frame that does arrive is rendered immediately.
     */
    private fun drainLoop() {
        val info = MediaCodec.BufferInfo()
        while (running) {
            val mc = codec ?: break
            try {
                when (val index = mc.dequeueOutputBuffer(info, DRAIN_TIMEOUT_US)) {
                    in 0..Int.MAX_VALUE -> {
                        // Present now on the device clock. A zero timestamp is
                        // outside SurfaceView's scheduling window and disables
                        // its ability to discard superseded frames at a VSYNC.
                        if (traceLatency) {
                            Log.i(TAG, "latency stage=decoded local_pts_us=${info.presentationTimeUs} decode_us=${System.nanoTime() / 1000 - info.presentationTimeUs}")
                        }
                        mc.releaseOutputBuffer(index, System.nanoTime())
                        if ((info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                            pendingInputs.updateAndGet { (it - 1).coerceAtLeast(0) }
                            framesDecoded.incrementAndGet()
                        }
                        outputPace.observe("pts_us=${info.presentationTimeUs} size=${info.size}")
                    }
                    MediaCodec.INFO_OUTPUT_FORMAT_CHANGED ->
                        Log.i(TAG, "output format now ${mc.outputFormat}")
                    else -> {} // INFO_TRY_AGAIN_LATER: nothing ready, loop again
                }
            } catch (e: IllegalStateException) {
                if (running) Log.e(TAG, "decoder drain failed", e)
                break
            }
        }
    }

    @Synchronized
    fun stop() {
        running = false
        drainThread?.join(500)
        drainThread = null
        codec?.let {
            runCatching { it.stop() }
            runCatching { it.release() }
        }
        codec = null
        pendingInputs.set(0)
        submittedPts.clear()
    }

    private companion object {
        const val TAG = "extraspace"
        const val INPUT_TIMEOUT_US = 10_000L

        /// Long enough that an idle desktop does not spin the CPU, short enough
        /// that shutdown is not noticeably delayed waiting for this to return.
        const val DRAIN_TIMEOUT_US = 20_000L
    }
}
