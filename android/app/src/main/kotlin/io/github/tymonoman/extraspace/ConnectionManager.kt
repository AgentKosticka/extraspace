package io.github.tymonoman.extraspace

import android.net.LocalServerSocket
import android.net.LocalSocket
import android.os.Build
import android.os.ParcelFileDescriptor
import java.io.FileInputStream
import java.io.FileOutputStream
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.Closeable
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * Owns the three abstract unix sockets the host connects to.
 *
 * The tablet listens and the host connects, rather than the other way round. That
 * ordering means the app can be launched and simply wait, and it survives the host
 * reconnecting without needing the app to be restarted.
 *
 * Each socket gets its own thread. They are almost entirely independent, and
 * keeping video off the same thread as control means a slow decode can never
 * delay a touch event.
 */
class ConnectionManager(
    private val callbacks: Callbacks,
    private val accessory: ParcelFileDescriptor? = null,
) : Closeable {

    interface Callbacks {
        /** Host has sent the stream parameters; set up the decoder. */
        fun onVideoConfig(width: Int, height: Int, framerate: Int)
        /** One access unit arrived. */
        fun onVideoFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean)
        /** Host asked us to start or stop the camera. */
        fun onCameraControl(enabled: Boolean, cameraId: String, width: Int, height: Int, framerate: Int, bitrateKbps: Int)
        fun onCursor(update: CursorUpdate)
        fun onConnected()
        fun onDisconnected(reason: String)
    }

    private val running = AtomicBoolean(false)
    // USB writes can block when the computer goes away. Keep them off the UI thread.
    private val outputQueue = ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS,
        ArrayBlockingQueue<Runnable>(16), { job -> Thread(job, "xs-usb-output") })
    private fun writeOutput(write: () -> Unit) {
        if (!running.get()) return
        if (accessory == null) { write(); return }
        try { outputQueue.execute { if (running.get()) write() } }
        catch (_: RejectedExecutionException) {
            if (running.get()) {
                close()
                callbacks.onDisconnected("USB output stalled; reconnect the computer")
            }
        }
    }
    private var controlServer: LocalServerSocket? = null
    private var videoServer: LocalServerSocket? = null
    private var cameraServer: LocalServerSocket? = null

    @Volatile private var controlSocket: LocalSocket? = null
    @Volatile private var videoSocket: LocalSocket? = null
    @Volatile private var cameraSocket: LocalSocket? = null

    @Volatile private var controlWriter: FrameWriter? = null
    @Volatile private var cameraWriter: FrameWriter? = null

    fun start() {
        if (!running.compareAndSet(false, true)) return
        if (accessory != null) {
            thread(name = "xs-accessory") { runAccessory(accessory) }
            return
        }
        try {
            controlServer = LocalServerSocket(Protocol.Sockets.CONTROL)
            videoServer = LocalServerSocket(Protocol.Sockets.VIDEO)
            cameraServer = LocalServerSocket(Protocol.Sockets.CAMERA)
        } catch (e: Exception) { close(); throw e }

        thread(name = "xs-control") { runControl() }
        thread(name = "xs-video") { runVideo() }
        thread(name = "xs-camera") { runCamera() }
        Log.i(TAG, "listening on all three sockets")
    }

    // ------------------------------------------------------------- control
    private fun runControl() {
        try {
            val socket = controlServer!!.accept()
            controlSocket = socket
            val reader = FrameReader(socket.inputStream)
            val writer = FrameWriter(socket.outputStream)
            controlWriter = writer

            sendHello(writer)
            callbacks.onConnected()

            while (running.get()) {
                val header = reader.readHeader()
                val payload = reader.readPayload(header)
                handleControl(header, payload, writer)
            }
        } catch (e: Exception) {
            if (running.get()) {
                Log.e(TAG, "control channel failed", e)
                callbacks.onDisconnected(e.message ?: "control channel closed")
            }
        }
    }

    private fun handleControl(header: FrameHeader, payload: ByteArray, writer: FrameWriter) {
        when (header.kind) {
            Protocol.ControlKind.HELLO_REQUEST -> sendHello(writer)
            Protocol.ControlKind.SESSION_END -> throw ProtocolException("Computer stopped displaying")
            Protocol.ControlKind.VIDEO_CONFIG -> {
                val json = JSONObject(String(payload))
                callbacks.onVideoConfig(
                    json.getInt("width"),
                    json.getInt("height"),
                    json.getInt("framerate"),
                )
            }
            Protocol.ControlKind.CAMERA_CONTROL -> {
                val json = JSONObject(String(payload))
                callbacks.onCameraControl(
                    json.getBoolean("enabled"),
                    json.optString("camera_id", "0"),
                    json.optInt("width", 1920),
                    json.optInt("height", 1080),
                    json.optInt("framerate", 30),
                    json.optInt("bitrate_kbps", 8000),
                )
            }
            Protocol.ControlKind.CURSOR -> {
                val update = CursorUpdate.decode(payload)
                if (update == null) {
                    Log.w(TAG, "malformed cursor update (${payload.size} bytes)")
                } else {
                    callbacks.onCursor(update)
                }
            }
            Protocol.ControlKind.PING -> {
                // Echo the timestamp back untouched so the host can measure
                // a true round trip without us needing a synced clock.
                writer.write(
                    Protocol.Channel.CONTROL, Protocol.ControlKind.PONG,
                    0, header.ptsUs, ByteArray(0),
                )
            }
            else -> Log.w(TAG, "unhandled control kind ${header.kind}")
        }
    }

    /** One USB bulk stream, multiplexed using the existing frame channel byte. */
    private fun runAccessory(fd: ParcelFileDescriptor) {
        try {
            val reader = FrameReader(AccessoryInput(FileInputStream(fd.fileDescriptor)))
            val writer = FrameWriter(AccessoryOutput(FileOutputStream(fd.fileDescriptor)))
            controlWriter = writer
            cameraWriter = writer
            sendHello(writer)
            callbacks.onConnected()
            var buffer = ByteArray(512 * 1024)
            while (running.get()) {
                val header = reader.readHeader()
                when (header.channel) {
                    Protocol.Channel.CONTROL -> handleControl(header, reader.readPayload(header), writer)
                    Protocol.Channel.VIDEO_DOWN -> {
                        if (header.length > buffer.size) buffer = ByteArray(header.length)
                        reader.readPayload(header, buffer)
                        callbacks.onVideoFrame(buffer, header.length, header.ptsUs,
                            header.flags.toInt() and Protocol.Flags.CODEC_CONFIG.toInt() != 0)
                    }
                    else -> throw ProtocolException("unexpected accessory channel ${header.channel}")
                }
            }
        } catch (e: Exception) {
            if (running.get()) callbacks.onDisconnected(e.message ?: "USB accessory disconnected")
        }
    }

    private fun sendHello(writer: FrameWriter) {
        val json = JSONObject().apply {
            put("protocol_version", Protocol.VERSION)
            put("device_id", DeviceInfo.deviceId)
            put("device_name", "${Build.MANUFACTURER} ${Build.MODEL}")
            put("android_release", Build.VERSION.RELEASE)
            put("width", callbacks.let { DeviceInfo.width })
            put("height", DeviceInfo.height)
            put("density_dpi", DeviceInfo.densityDpi)
            put("refresh_rate", DeviceInfo.refreshRate)
            put("cameras", JSONArray().apply {
                DeviceInfo.cameras.forEach { cam ->
                    put(JSONObject().apply {
                        put("id", cam.id)
                        put("facing", cam.facing)
                        put("max_width", cam.maxWidth)
                        put("max_height", cam.maxHeight)
                    })
                }
            })
        }
        writer.write(
            Protocol.Channel.CONTROL, Protocol.ControlKind.HELLO,
            0, 0, json.toString().toByteArray(),
        )
        Log.i(TAG, "sent hello: ${DeviceInfo.width}x${DeviceInfo.height} @${DeviceInfo.refreshRate}")
    }

    /** Sends a touch event. Cheap enough to call straight from the input thread. */
    fun sendTouch(event: TouchEvent) {
        val writer = controlWriter ?: return
        writeOutput {
            try {
                writer.write(
                    Protocol.Channel.TOUCH, 0, 0,
                    System.nanoTime() / 1000, event.encode(),
                )
            } catch (e: Exception) {
                if (running.get()) Log.w(TAG, "could not send touch", e)
            }
        }
    }

    /** Periodic health report that drives the host's adaptive bitrate controller. */
    fun sendStats(queueDepth: Int, decoded: Long, dropped: Long, lastPtsUs: Long, renderedAtUs: Long) {
        val writer = controlWriter ?: return
        val json = JSONObject().apply {
            put("decode_queue_depth", queueDepth)
            put("frames_decoded", decoded)
            put("frames_dropped", dropped)
            put("last_frame_pts_us", lastPtsUs)
            put("rendered_at_us", renderedAtUs)
        }
        writeOutput {
            try {
                writer.write(
                    Protocol.Channel.CONTROL, Protocol.ControlKind.STATS,
                    0, System.nanoTime() / 1000, json.toString().toByteArray(),
                )
            } catch (e: Exception) {
                if (running.get()) Log.w(TAG, "could not send stats", e)
            }
        }
    }

    // --------------------------------------------------------------- video
    private fun runVideo() {
        try {
            val socket = videoServer!!.accept()
            videoSocket = socket
            val reader = FrameReader(socket.inputStream)
            // One reusable buffer: at 60fps, allocating per frame would keep the
            // GC busy for no reason.
            var buf = ByteArray(512 * 1024)
            val recvPace = PaceWatch("recv")

            while (running.get()) {
                val header = reader.readHeader()
                if (header.length > buf.size) buf = ByteArray(header.length.coerceAtLeast(buf.size * 2))
                reader.readPayload(header, buf)
                val isConfig = (header.flags.toInt() and Protocol.Flags.CODEC_CONFIG.toInt()) != 0
                recvPace.observe("bytes=${header.length} pts_us=${header.ptsUs}")
                callbacks.onVideoFrame(buf, header.length, header.ptsUs, isConfig)
            }
        } catch (e: Exception) {
            if (running.get()) {
                Log.e(TAG, "video channel failed", e)
                callbacks.onDisconnected(e.message ?: "video channel closed")
            }
        }
    }

    // -------------------------------------------------------------- camera
    private fun runCamera() {
        try {
            val socket = cameraServer!!.accept()
            cameraSocket = socket
            cameraWriter = FrameWriter(socket.outputStream)
            Log.i(TAG, "camera channel connected")
            // Host-bound only; nothing to read. Park until shutdown so the
            // socket stays open.
            while (running.get()) Thread.sleep(1000)
        } catch (e: Exception) {
            if (running.get()) Log.e(TAG, "camera channel failed", e)
        }
    }

    /** Sends one encoded camera access unit to the host. */
    fun sendCameraFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean, isKeyframe: Boolean) {
        val writer = cameraWriter ?: return
        val owned = if (accessory != null) data.copyOf(length) else data
        var flags = 0
        if (isConfig) flags = flags or Protocol.Flags.CODEC_CONFIG.toInt()
        if (isKeyframe) flags = flags or Protocol.Flags.KEYFRAME.toInt()
        writeOutput {
            try {
                writer.write(Protocol.Channel.CAMERA_UP, 0, flags.toShort(), ptsUs, owned, length)
            } catch (e: Exception) {
                if (running.get()) Log.w(TAG, "could not send camera frame", e)
            }
        }
    }

    override fun close() {
        running.set(false)
        outputQueue.shutdownNow()
        listOf<Closeable?>(controlServer, videoServer, cameraServer, controlSocket, videoSocket, cameraSocket, accessory)
            .forEach { runCatching { it?.close() } }
        controlWriter = null
        cameraWriter = null
    }

    private companion object {
        const val TAG = "extraspace"
    }
}
