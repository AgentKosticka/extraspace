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
 * decoded. Compressed inputs must retain their reference chain. Backpressure is
 * bounded; failure ends the connection so the host restarts with a fresh IDR.
 * Only decoded outputs may be discarded to reduce presentation latency.
 */
class VideoDecoder(private val surface: Surface, private val traceLatency: Boolean = false,
    @Volatile var selectNewestFrame: Boolean = false,
    private val onFailure: (String) -> Unit = {}) {
    @Volatile private var codec: MediaCodec? = null
    @Volatile private var running = false
    private var drainThread: Thread? = null
    private var drainWake: DecoderWake? = null

    /** Codec input queue depth -- the host's main signal that we are falling behind. */
    val queueDepth: Int get() = pendingInputs.get()
    val framesDecoded = AtomicLong(0)
    val framesDropped = AtomicLong(0)
    /** Device-clock microseconds reported by the codec for its last surface render. */
    val renderedAtUs = AtomicLong(0)
    val lastFramePtsUs = AtomicLong(0)

    /** Frames submitted but not yet released for display. Touched by two threads. */
    private val pendingInputs = AtomicInteger(0)
    private val inputPace = PaceWatch("decode_in", traceLatency)
    private val outputPace = PaceWatch("decode_out", traceLatency)
    @Volatile private var presentation: PresentationWatch? = null
    fun presentationFailure(): String? = presentation?.failure(System.nanoTime(), framesDecoded.get(), pendingInputs.get())
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

        val created = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        codec = created
        try {
            created.apply {
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
        } catch (e: Exception) {
            codec = null
            runCatching { created.release() }
            throw e
        }
        presentation = PresentationWatch(System.nanoTime())
        framesDecoded.set(0)
        framesDropped.set(0)
        running = true
        // Output is drained on its own thread rather than piggybacking on input.
        // Draining only when a new frame arrives means that when the desktop goes
        // idle -- which, since mutter only sends on damage, is most of the time --
        // the last frame sits decoded but unrendered until something else changes.
        val activeCodec = codec ?: error("Decoder creation failed")
        val wake = DecoderWake()
        drainWake = wake
        drainThread = thread(name = "xs-decode-drain") { drainLoop(activeCodec, wake) }
        Log.i(TAG, "decoder started ${width}x$height low-latency=${Build.VERSION.SDK_INT >= Build.VERSION_CODES.R}")
    }

    /**
     * Submits one complete access unit or throws to restart the stream. Continuing
     * after losing a compressed input could corrupt every dependent picture.
     */
    @Synchronized
    fun decode(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean) {
        val mc = codec ?: throw ProtocolException("Decoder is unavailable; restart the stream")
        if (!running) throw ProtocolException("Decoder stopped; restart the stream")
        try {
            // Allow the drain thread to free an input. A bounded stall is safer
            // than silently dropping an H.264 reference picture.
            val waitStart = System.nanoTime()
            val index = DecoderInput.fill(data, length,
                dequeue = { mc.dequeueInputBuffer(INPUT_TIMEOUT_US) },
                buffer = { mc.getInputBuffer(it) })
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
            } catch (e: RuntimeException) {
                if (!isConfig) {
                    submittedPts.remove(localPtsUs)
                    pendingInputs.updateAndGet { (it - 1).coerceAtLeast(0) }
                }
                throw e
            }
            drainWake?.signal()
            if (traceLatency) inputPace.observe {
                "bytes=$length pts_us=$ptsUs wait_ms=${(System.nanoTime() - waitStart) / 1_000_000L}"
            }
        } catch (e: RuntimeException) {
            throw ProtocolException("Decoder rejected input; restart the stream").apply { initCause(e) }
        }
    }

    /**
     * Releases finished frames to the surface as soon as they are ready.
     *
     * Blocks on the codec rather than spinning, so an idle desktop costs nothing
     * while a frame that does arrive is rendered immediately.
     */
    private fun drainLoop(mc: MediaCodec, wake: DecoderWake) {
        val info = MediaCodec.BufferInfo()
        while (running && codec === mc) {
            // Once all submitted pictures have been released, only a new input
            // can produce output. Signal after queueing, retaining early signals.
            if (!wake.awaitWork { pendingInputs.get() > 0 }) break
            try {
                when (val index = mc.dequeueOutputBuffer(info, DRAIN_TIMEOUT_US)) {
                    in 0..Int.MAX_VALUE -> {
                        var newestIndex = index
                        var newestInfo = info
                        // Drop only decoded outputs, preserving all H.264 reference inputs.
                        // A bounded drain avoids starving presentation on fast producers.
                        if (selectNewestFrame) {
                            var inspected = 0
                            while (inspected++ < 8 && running) {
                                val nextInfo = MediaCodec.BufferInfo()
                                val next = mc.dequeueOutputBuffer(nextInfo, 0)
                                if (next == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) continue
                                if (next < 0) break
                                releaseDecoded(mc, newestIndex, newestInfo, false)
                                newestIndex = next
                                newestInfo = nextInfo
                            }
                        }
                        releaseDecoded(mc, newestIndex, newestInfo, true)
                    }
                    MediaCodec.INFO_OUTPUT_FORMAT_CHANGED ->
                        Log.i(TAG, "output format now ${mc.outputFormat}")
                    else -> {} // INFO_TRY_AGAIN_LATER: nothing ready, loop again
                }
            } catch (e: RuntimeException) {
                if (running && codec === mc) {
                    running = false
                    Log.e(TAG, "decoder drain failed", e)
                    onFailure("Decoder output failed; restart the stream for a fresh keyframe")
                }
                break
            }
        }
    }

    private fun releaseDecoded(mc: MediaCodec, index: Int, info: MediaCodec.BufferInfo, render: Boolean) {
        if (traceLatency) {
            Log.i(TAG, "latency stage=decoded local_pts_us=${info.presentationTimeUs} decode_us=${System.nanoTime() / 1000 - info.presentationTimeUs} render=$render")
        }
        presentation?.lastOutputNs = System.nanoTime()
        if (render) mc.releaseOutputBuffer(index, System.nanoTime())
        else { mc.releaseOutputBuffer(index, false); submittedPts.remove(info.presentationTimeUs) }
        if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
            pendingInputs.updateAndGet { (it - 1).coerceAtLeast(0) }
            if (render) framesDecoded.incrementAndGet() else framesDropped.incrementAndGet()
        }
        if (traceLatency) outputPace.observe { "pts_us=${info.presentationTimeUs} size=${info.size}" }
    }

    @Synchronized
    fun stop() {
        running = false
        drainWake?.close()
        drainWake = null
        drainThread?.join(500)
        drainThread = null
        codec?.let {
            runCatching { it.stop() }
            runCatching { it.release() }
        }
        codec = null
        presentation = null
        pendingInputs.set(0)
        submittedPts.clear()
    }

    private companion object {
        const val TAG = "extraspace"
        const val INPUT_TIMEOUT_US = 250_000L

        // This is a maximum wait, not a presentation delay: the codec wakes the
        // call as soon as an output is ready. With no pending inputs DecoderWake
        // parks indefinitely; stop() wakes it within the existing shutdown budget.
        const val DRAIN_TIMEOUT_US = 250_000L
    }
}
