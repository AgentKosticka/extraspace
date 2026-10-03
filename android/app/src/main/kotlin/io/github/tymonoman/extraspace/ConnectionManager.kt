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
import java.util.concurrent.TimeUnit
import java.util.concurrent.CountDownLatch
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
    private val transportMode: TransportMode = TransportMode.AUTO,
    private val traceLatency: Boolean = false,
) : Closeable {

    interface Callbacks {
        /** Host has sent the stream parameters; set up the decoder. */
        fun onVideoConfig(width: Int, height: Int, framerate: Int)
        /** One access unit arrived. */
        fun onVideoFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean)
        /** Host asked us to start or stop the camera. */
        fun onCameraControl(enabled: Boolean, cameraId: String, width: Int, height: Int, framerate: Int, bitrateKbps: Int, generation: Long)
        fun onCursor(update: CursorUpdate)
        fun onConnected()
        fun onDisconnected(reason: String)
    }

    private val lifecycleLock = Any()
    private val running = ConnectionState()
    private val videoReady = CountDownLatch(1)
    private var selectedTransport: TransportMode? = null
    // Keep both ADB and accessory writes off the main and reader threads.
    private val outputQueue = OutputDispatcher("xs-control-output", ::fail)
    private val cameraQueue = if (accessory == null) OutputDispatcher("xs-camera-output", ::fail) else outputQueue
    private fun writeOutput(key: String? = null, write: () -> Unit) {
        if (running.get()) outputQueue.submit(key) { if (running.get()) write() }
    }
    @Volatile private var cameraGeneration = 0L
    private var controlServer: LocalServerSocket? = null
    private var videoServer: LocalServerSocket? = null
    private var cameraServer: LocalServerSocket? = null

    @Volatile private var controlSocket: LocalSocket? = null
    @Volatile private var videoSocket: LocalSocket? = null
    @Volatile private var cameraSocket: LocalSocket? = null

    @Volatile private var controlWriter: FrameWriter? = null
    @Volatile private var cameraWriter: FrameWriter? = null

    fun start() {
        synchronized(lifecycleLock) {
            if (!running.start()) return
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
    }

    private fun accept(server: LocalServerSocket, publish: (LocalSocket) -> Unit): LocalSocket {
        val socket = server.accept()
        synchronized(lifecycleLock) {
            if (!running.get()) {
                socket.close()
                throw ProtocolException("Connection has closed")
            }
            publish(socket)
        }
        return socket
    }

    // ------------------------------------------------------------- control
    private fun runControl() {
        try {
            val socket = accept(controlServer!!) { controlSocket = it }
            val reader = FrameReader(socket.inputStream)
            val writer = FrameWriter(socket.outputStream)
            controlWriter = writer

            sendHello(writer)

            while (running.get()) {
                val header = reader.readHeader()
                val payload = reader.readPayload(header)
                handleControl(header, payload, writer)
            }
        } catch (e: Exception) {
            if (running.get()) {
                Log.e(TAG, "control channel failed", e)
                fail(e.message ?: "control channel closed")
            }
        }
    }

    private fun handleControl(header: FrameHeader, payload: ByteArray, writer: FrameWriter) {
        when (header.kind) {
            Protocol.ControlKind.HELLO_REQUEST -> {
                val host = if (payload.isEmpty()) TransportMode.AUTO else
                    TransportMode.parse(JSONObject(String(payload)).getString("transport"))
                // Hello always advertises the local selector, including when
                // discovery arrived on a method that cannot carry the stream.
                sendHello(writer)
                selectedTransport = host.selectForLink(transportMode,
                    if (accessory == null) TransportMode.ADB else TransportMode.ACCESSORY)
                callbacks.onConnected()
            }
            Protocol.ControlKind.ERROR -> throw ProtocolException(String(payload))
            Protocol.ControlKind.SESSION_END -> throw ProtocolException("Computer stopped displaying")
            Protocol.ControlKind.VIDEO_CONFIG -> {
                val actual = if (accessory == null) TransportMode.ADB else TransportMode.ACCESSORY
                val selected = selectedTransport ?: transportMode
                if (!transportMode.allows(actual) || (selected != TransportMode.AUTO && selected != actual)) {
                    throw ProtocolException("Incompatible connection methods: the computer used an unselected method. Select a shared method or Automatic in both apps.")
                }
                val json = JSONObject(String(payload))
                callbacks.onVideoConfig(
                    json.getInt("width"),
                    json.getInt("height"),
                    json.getInt("framerate"),
                )
                videoReady.countDown()
            }
            Protocol.ControlKind.CAMERA_CONTROL -> {
                val json = JSONObject(String(payload))
                cameraGeneration++
                callbacks.onCameraControl(
                    json.getBoolean("enabled"),
                    json.optString("camera_id", "0"),
                    json.optInt("width", 1920),
                    json.optInt("height", 1080),
                    json.optInt("framerate", 30),
                    json.optInt("bitrate_kbps", 8000),
                    cameraGeneration,
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
                writeOutput { writer.write(
                    Protocol.Channel.CONTROL, Protocol.ControlKind.PONG,
                    0, header.ptsUs, ByteArray(0),
                ) }
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
            // AOA is host-requested, including reconnects with the cable attached.
            // ADB announces itself when its control socket connects.
            var buffer = ByteArray(512 * 1024)
            while (running.get()) {
                val header = reader.readHeader()
                when (header.channel) {
                    Protocol.Channel.CONTROL -> handleControl(header, reader.readPayload(header), writer)
                    Protocol.Channel.VIDEO_DOWN -> {
                        if (header.length > buffer.size) buffer = ByteArray(header.length)
                        reader.readPayload(header, buffer)
                        deliverVideoFrame(buffer, header.length, header.ptsUs,
                            header.flags.toInt() and Protocol.Flags.CODEC_CONFIG.toInt() != 0)
                    }
                    else -> throw ProtocolException("unexpected accessory channel ${header.channel}")
                }
            }
        } catch (e: Exception) {
            fail(e.message ?: "USB accessory disconnected")
        }
    }

    private fun deliverVideoFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean) {
        if (!videoReady.await(5, TimeUnit.SECONDS)) throw ProtocolException("Video configuration timed out")
        if (running.get()) callbacks.onVideoFrame(data, length, ptsUs, isConfig)
    }

    private fun sendHello(writer: FrameWriter) {
        val json = JSONObject().apply {
            put("protocol_version", Protocol.VERSION)
            put("transport", transportMode.wireName)
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
        val key = if (event.action == Protocol.TouchAction.MOTION) "motion:${event.slot}" else null
        writeOutput(key) {
            try {
                writer.write(
                    Protocol.Channel.TOUCH, 0, 0,
                    System.nanoTime() / 1000, event.encode(),
                )
            } catch (e: Exception) {
                fail(e.message ?: "could not send touch")
            }
        }
    }

    fun sendCameraStatus(state: String, message: String, generation: Long = cameraGeneration) {
        val writer = controlWriter ?: return
        val json = JSONObject().put("state", state).put("message", message).toString().toByteArray()
        writeOutput {
            if (generation == cameraGeneration) writer.write(Protocol.Channel.CONTROL, Protocol.ControlKind.CAMERA_STATUS, 0, 0, json)
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
        writeOutput("stats") {
            try {
                writer.write(
                    Protocol.Channel.CONTROL, Protocol.ControlKind.STATS,
                    0, System.nanoTime() / 1000, json.toString().toByteArray(),
                )
            } catch (e: Exception) {
                fail(e.message ?: "could not send stats")
            }
        }
    }

    // --------------------------------------------------------------- video
    private fun runVideo() {
        try {
            val socket = accept(videoServer!!) { videoSocket = it }
            val reader = FrameReader(socket.inputStream)
            // One reusable buffer: at 60fps, allocating per frame would keep the
            // GC busy for no reason.
            var buf = ByteArray(512 * 1024)
            val recvPace = PaceWatch("recv", traceLatency)

            while (running.get()) {
                val header = reader.readHeader()
                if (header.length > buf.size) buf = ByteArray(header.length.coerceAtLeast(buf.size * 2))
                reader.readPayload(header, buf)
                val isConfig = (header.flags.toInt() and Protocol.Flags.CODEC_CONFIG.toInt()) != 0
                if (traceLatency) recvPace.observe { "bytes=${header.length} pts_us=${header.ptsUs}" }
                deliverVideoFrame(buf, header.length, header.ptsUs, isConfig)
            }
        } catch (e: Exception) {
            if (running.get()) {
                Log.e(TAG, "video channel failed", e)
                fail(e.message ?: "video channel closed")
            }
        }
    }

    // -------------------------------------------------------------- camera
    private fun runCamera() {
        try {
            val socket = accept(cameraServer!!) { cameraSocket = it }
            cameraWriter = FrameWriter(socket.outputStream)
            Log.i(TAG, "camera channel connected")
            // Host-bound only. Block until EOF/close instead of waking every
            // second; a disconnected host is detected immediately.
            if (socket.inputStream.read() != -1) throw ProtocolException("Unexpected camera channel input")
            if (running.get()) fail("camera channel closed")
        } catch (e: Exception) {
            fail(e.message ?: "camera channel closed")
        }
    }

    /** Sends one encoded camera access unit to the host. */
    fun sendCameraFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean, isKeyframe: Boolean, generation: Long) {
        val writer = cameraWriter ?: return
        val owned = data.copyOf(length)
        var flags = 0
        if (isConfig) flags = flags or Protocol.Flags.CODEC_CONFIG.toInt()
        if (isKeyframe) flags = flags or Protocol.Flags.KEYFRAME.toInt()
        cameraQueue.submit {
            if (!running.get() || generation != cameraGeneration) return@submit
            try {
                writer.write(Protocol.Channel.CAMERA_UP, 0, flags.toShort(), ptsUs, owned, length)
            } catch (e: Exception) {
                fail(e.message ?: "could not send camera frame")
            }
        }
    }

    fun fail(reason: String) {
        if (!running.finish()) return
        closeResources()
        callbacks.onDisconnected(reason)
    }

    override fun close() {
        running.finish()
        closeResources()
    }

    private fun closeResources() = synchronized(lifecycleLock) {
        videoReady.countDown()
        outputQueue.close()
        if (cameraQueue !== outputQueue) cameraQueue.close()
        // Closing a descriptor alone can leave another thread's blocking read
        // alive. Shut down both directions first to wake readers/writers and
        // deliver EOF to the host, then release the descriptors.
        listOf(controlSocket, videoSocket, cameraSocket).forEach { socket ->
            runCatching { socket?.shutdownInput() }
            runCatching { socket?.shutdownOutput() }
            runCatching { socket?.close() }
        }
        listOf<Closeable?>(controlServer, videoServer, cameraServer, accessory)
            .forEach { runCatching { it?.close() } }
        controlWriter = null
        cameraWriter = null
    }

    private companion object {
        const val TAG = "extraspace"
    }
}
