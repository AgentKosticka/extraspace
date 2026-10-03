package io.github.tymonoman.extraspace

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.util.Range
import android.view.Surface
import androidx.core.content.ContextCompat
import kotlin.concurrent.thread

/**
 * Camera2 capture encoded straight to H.264 and streamed to the host, which
 * writes it into a v4l2loopback device so it appears as an ordinary webcam.
 *
 * The camera renders directly into the encoder's input surface, so frames never
 * touch the CPU or the Java heap on the way through.
 */
class CameraSource(
    private val context: Context,
    private val onStatus: (String, String) -> Unit,
    private val onFrame: (data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean, isKeyframe: Boolean) -> Unit,
) {
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    @Volatile private var encoder: MediaCodec? = null
    private var inputSurface: Surface? = null
    private var thread: HandlerThread? = null
    @Volatile private var handler: Handler? = null
    @Volatile private var running = false
    private var drainThread: Thread? = null
    private var epoch = 0

    fun hasPermission(): Boolean =
        ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
            PackageManager.PERMISSION_GRANTED

    @Synchronized
    @SuppressLint("MissingPermission")
    fun start(cameraId: String, width: Int, height: Int, framerate: Int, bitrateKbps: Int) {
        if (!hasPermission()) {
            Log.e(TAG, "camera permission not granted; ask for it before starting")
            return
        }
        stop()
        running = true

        thread = HandlerThread("xs-camera").also { it.start() }
        handler = Handler(thread!!.looper)

        val activeEpoch = epoch
        try {
            val manager = context.getSystemService(CameraManager::class.java)
            val chars = manager.getCameraCharacteristics(cameraId)
            val map = chars.get(android.hardware.camera2.CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
                ?: error("Camera has no supported video modes")
            val sizes = map.getOutputSizes(MediaCodec::class.java)?.toList().orEmpty()
            val codecs = android.media.MediaCodecList(android.media.MediaCodecList.REGULAR_CODECS).codecInfos
                .filter { it.isEncoder && it.supportedTypes.contains(MediaFormat.MIMETYPE_VIDEO_AVC) }
                .filter { codec -> runCatching {
                    val caps = codec.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC)
                    caps.colorFormats.contains(MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface) &&
                        listOf(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR,
                            MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR).any { caps.encoderCapabilities.isBitrateModeSupported(it) }
                }.getOrDefault(false) }
                .sortedBy { !it.isHardwareAccelerated }
            val rates = chars.get(android.hardware.camera2.CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)?.toList().orEmpty()
            val preferredRates = rates.filter { it.upper <= framerate }.sortedByDescending { it.upper }
                .ifEmpty { rates.sortedBy { it.upper } }
            // Match Camera2 frame duration, resolution and H.264 encoder support.
            val mode = preferredRates.firstNotNullOfOrNull { range ->
                val choices = sizes.filter { size ->
                    val duration = map.getOutputMinFrameDuration(MediaCodec::class.java, size)
                    size.width % 2 == 0 && size.height % 2 == 0 &&
                        (duration == 0L || duration <= 1_000_000_000L / range.upper) &&
                        codecs.any { codec -> runCatching {
                            codec.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC).videoCapabilities
                                .areSizeAndRateSupported(size.width, size.height, range.upper.toDouble())
                        }.getOrDefault(false) }
                }
                val size = choices.filter { it.width <= width && it.height <= height }
                    .maxByOrNull { it.width.toLong() * it.height }
                    ?: choices.minByOrNull { it.width.toLong() * it.height }
                size?.let { it to range }
            } ?: error("Camera and H.264 encoder have no shared video mode")
            val (size, selectedRange) = mode
            val fps = selectedRange.upper
            val selectedCodec = codecs.first { codec -> runCatching {
                codec.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC).videoCapabilities
                    .areSizeAndRateSupported(size.width, size.height, fps.toDouble())
            }.getOrDefault(false) }
            startEncoder(size.width, size.height, fps, bitrateKbps, selectedCodec)

            manager.openCamera(cameraId, object : CameraDevice.StateCallback() {
                override fun onOpened(cam: CameraDevice) {
                    synchronized(this@CameraSource) {
                        if (!running || epoch != activeEpoch) { cam.close(); return }
                        device = cam
                        runCatching { createSession(cam, selectedRange, activeEpoch) }
                            .onFailure { cameraFailed(it.message ?: "Could not create camera session") }
                    }
                }

                override fun onDisconnected(cam: CameraDevice) {
                    Log.w(TAG, "camera disconnected")
                    synchronized(this@CameraSource) {
                        cam.close()
                        if (epoch == activeEpoch) cameraFailed("Camera disconnected or unavailable")
                    }
                }

                override fun onError(cam: CameraDevice, error: Int) {
                    Log.e(TAG, "camera error $error")
                    synchronized(this@CameraSource) {
                        cam.close()
                        if (epoch == activeEpoch) cameraFailed("Camera error $error")
                    }
                }
            }, handler)
        } catch (e: Exception) {
            cameraFailed(e.message ?: "Could not start camera")
        }
    }

    private fun startEncoder(width: Int, height: Int, framerate: Int, bitrateKbps: Int, selected: MediaCodecInfo) {
        val capabilities = selected.getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC)
        val rateControl = listOf(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR,
            MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR).firstOrNull {
            capabilities.encoderCapabilities.isBitrateModeSupported(it)
        } ?: error("Camera encoder supports neither constant nor variable bitrate")
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height).apply {
            setInteger(
                MediaFormat.KEY_COLOR_FORMAT,
                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface,
            )
            setInteger(MediaFormat.KEY_BIT_RATE, capabilities.videoCapabilities.bitrateRange.clamp(bitrateKbps * 1000))
            setInteger(MediaFormat.KEY_FRAME_RATE, framerate)
            // A keyframe every second: a webcam consumer may attach at any moment
            // and cannot show anything until it sees one.
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            setInteger(MediaFormat.KEY_BITRATE_MODE, rateControl)
        }

        // Publish before configuration so a partial start is released by stop().
        val mc = MediaCodec.createByCodecName(selected.name)
        encoder = mc
        mc.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        inputSurface = mc.createInputSurface()
        mc.start()
        drainThread = thread(name = "xs-camera-drain") { drainEncoder(mc) }
        Log.i(TAG, "camera encoder started ${width}x$height @$framerate ${bitrateKbps}kbps")
    }

    private fun createSession(cam: CameraDevice, fpsRange: Range<Int>, activeEpoch: Int) {
        val surface = inputSurface ?: return
        val request = cam.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
            addTarget(surface)
            set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, fpsRange)
            val manager = context.getSystemService(CameraManager::class.java)
            val supported = manager.getCameraCharacteristics(cam.id)
                .get(android.hardware.camera2.CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES) ?: intArrayOf()
            val focus = listOf(CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO,
                CaptureRequest.CONTROL_AF_MODE_AUTO, CaptureRequest.CONTROL_AF_MODE_OFF)
                .firstOrNull { it in supported }
            if (focus != null) set(CaptureRequest.CONTROL_AF_MODE, focus)
        }.build()

        @Suppress("DEPRECATION")
        cam.createCaptureSession(listOf(surface), object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(s: CameraCaptureSession) {
                synchronized(this@CameraSource) {
                    if (!running || epoch != activeEpoch) { s.close(); return }
                    session = s
                    runCatching { s.setRepeatingRequest(request, null, handler) }
                        .onSuccess { onStatus("running", context.getString(R.string.camera_running)) }
                        .onFailure { cameraFailed(it.message ?: "Could not start camera capture") }
                }
            }

            override fun onConfigureFailed(s: CameraCaptureSession) {
                Log.e(TAG, "capture session configuration failed")
                synchronized(this@CameraSource) {
                    s.close()
                    if (epoch == activeEpoch) cameraFailed("Camera capture configuration failed")
                }
            }
        }, handler)
    }

    private fun drainEncoder(mc: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        // Keep this worker bound to its codec, including across quick restarts.
        while (running && encoder === mc) {
            try {
                val index = mc.dequeueOutputBuffer(info, DRAIN_TIMEOUT_US)
                if (index < 0) continue
                val buffer = mc.getOutputBuffer(index)
                if (buffer != null && info.size > 0) {
                    val data = ByteArray(info.size)
                    buffer.position(info.offset)
                    buffer.get(data, 0, info.size)
                    val isConfig = (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0
                    val isKey = (info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME) != 0
                    onFrame(data, info.size, info.presentationTimeUs, isConfig, isKey)
                }
                mc.releaseOutputBuffer(index, false)
            } catch (e: IllegalStateException) {
                if (running && encoder === mc) {
                    Log.e(TAG, "encoder drain failed", e)
                    handler?.post {
                        synchronized(this@CameraSource) { if (encoder === mc) cameraFailed(e.message ?: "Camera encoder failed") }
                    }
                }
                break
            }
        }
    }

    private fun cameraFailed(reason: String) {
        stop()
        onStatus("failed", context.getString(R.string.camera_failed, reason))
    }

    @Synchronized
    fun stop() {
        running = false
        epoch++
        runCatching { session?.stopRepeating() }
        runCatching { session?.close() }
        runCatching { device?.close() }
        drainThread?.join(500)
        drainThread = null
        runCatching { encoder?.stop() }
        runCatching { encoder?.release() }
        runCatching { inputSurface?.release() }
        thread?.quitSafely()
        session = null
        device = null
        encoder = null
        inputSurface = null
        thread = null
        handler = null
    }

    private companion object {
        const val TAG = "extraspace"
        // The codec wakes as soon as output is ready; this only limits idle
        // timeout wakeups (100/s -> 4/s), with the same 500 ms stop budget.
        const val DRAIN_TIMEOUT_US = 250_000L
    }
}
