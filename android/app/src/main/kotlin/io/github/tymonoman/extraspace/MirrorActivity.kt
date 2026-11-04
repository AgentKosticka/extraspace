package io.github.tymonoman.extraspace

import android.annotation.SuppressLint
import android.graphics.SurfaceTexture
import android.os.Build
import android.os.Bundle
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.content.Intent
import android.content.pm.PackageManager
import android.Manifest
import android.widget.Button
import android.widget.Spinner
import android.widget.ArrayAdapter
import android.widget.AdapterView
import androidx.appcompat.widget.SwitchCompat
import android.app.AlertDialog
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.addCallback
import androidx.core.content.ContextCompat
import android.util.Log
import android.view.InputDevice
import android.view.MotionEvent
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.TextureView
import android.view.View
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

/**
 * A direct decoder surface with the host cursor in the app layer above it.
 * TextureView remains available for devices with broken surface composition.
 */
class MirrorActivity : AppCompatActivity(), ConnectionManager.Callbacks {

    private lateinit var videoView: View
    private var textureView: TextureView? = null
    private var surfaceView: SurfaceView? = null
    private var ownsDecoderSurface = false
    private lateinit var statusView: TextView
    private lateinit var cursorOverlay: CursorOverlay
    private val ink by lazy { findViewById<InkOverlay>(R.id.ink) }
    private var localInk = true
    @Volatile private var decoder: VideoDecoder? = null
    private data class VideoSession(val epoch: Int, val decoder: VideoDecoder)
    @Volatile private var videoSession: VideoSession? = null
    @Volatile private var connectionEpoch = 0
    private val touches = TouchTracker()
    private val stylus = StylusTracker()
    private var decoderSurface: Surface? = null
    @Volatile private var connection: ConnectionManager? = null
    private var camera: CameraSource? = null
    private lateinit var lobby: View
    private lateinit var accessories: AccessoryController
    private val preferences by lazy { getSharedPreferences("companion", MODE_PRIVATE) }
    private var pendingAccessory: ParcelFileDescriptor? = null
    private var accessoryConnected = false
    private var destroyed = false
    private var waitingCamera: (() -> Unit)? = null
    private val cameraPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { allowed ->
        val start = waitingCamera; waitingCamera = null
        if (allowed) start?.invoke()
        else {
            val message = getString(R.string.camera_denied)
            connection?.sendCameraStatus("failed", message)
            showStatus(message)
        }
    }
    private val main = Handler(Looper.getMainLooper())

    /** Dimensions of the incoming stream; touches are mapped into this space. */
    private var streamWidth = 0
    private var streamHeight = 0
    private var streamFramerate = 60

    private val statsTicker = object : Runnable {
        override fun run() {
            decoder?.let { d ->
                d.presentationFailure()?.let { reason -> connection?.fail(reason); return }
                connection?.sendStats(
                    d.queueDepth,
                    d.framesDecoded.get(),
                    d.framesDropped.get(),
                    d.lastFramePtsUs.get(),
                    d.renderedAtUs.get(),
                    d.decodeTimeUs.get(),
                )
            }
            main.postDelayed(this, STATS_INTERVAL_MS)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        DeviceInfo.load(this)

        // A second monitor that sleeps is not a second monitor.
        // Keep awake only while streaming; the setup screen can sleep.
        goFullscreen()

        setContentView(R.layout.activity_mirror)
        // Android batches touch and stylus motion to one event per vsync by
        // default, holding samples for up to a frame. Remote input wants each
        // panel sample as it arrives; the output queue coalesces bursts.
        window.decorView.requestUnbufferedDispatch(InputDevice.SOURCE_CLASS_POINTER)
        statusView = findViewById(R.id.status)
        lobby = findViewById(R.id.lobby)
        // Match the desktop's readable preferences column on wide tablets.
        lobby.addOnLayoutChangeListener { _, left, _, right, _, oldLeft, _, oldRight, _ ->
            if (right - left != oldRight - oldLeft) {
                val content = findViewById<View>(R.id.lobby_content)
                val params = content.layoutParams as FrameLayout.LayoutParams
                params.width = minOf(right - left, (640 * resources.displayMetrics.density).toInt())
                params.gravity = android.view.Gravity.CENTER_HORIZONTAL
                content.layoutParams = params
            }
        }
        setupLobby()
        accessories = AccessoryController(this,
            // Keep discovery available so a disallowed link can report why the
            // selectors are incompatible. ConnectionManager gates streaming.
            enabled = { !destroyed },
            status = { showStatus(it) },
            ready = { fd ->
                resetConnection()
                pendingAccessory = fd
                accessoryConnected = true
                startConnection()
            },
            detached = { resetConnection(); accessories.reset(); startConnection() })
        onBackPressedDispatcher.addCallback(this) { showLobby() }
        main.post { accessories.check() }
        val original = findViewById<SurfaceView>(R.id.surface)
        // Retain a diagnostic fallback for devices with broken surface composition.
        if (intent.getBooleanExtra("texture_output", false)) {
            val parent = original.parent as FrameLayout
            parent.removeView(original)
            val texture = TextureView(this)
            parent.addView(texture, 0, original.layoutParams)
            textureView = texture
            videoView = texture
            texture.isOpaque = true
            texture.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                override fun onSurfaceTextureAvailable(texture: SurfaceTexture, width: Int, height: Int) {
                    if (streamWidth > 0) texture.setDefaultBufferSize(streamWidth, streamHeight)
                    attachDecoder(Surface(texture), true)
                }
                override fun onSurfaceTextureSizeChanged(texture: SurfaceTexture, width: Int, height: Int) = Unit
                override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
                    stopDecoder()
                    return true
                }
                override fun onSurfaceTextureUpdated(texture: SurfaceTexture) {
                    if (intent.getBooleanExtra("latency_trace", false) && texture.timestamp > 0) {
                        Log.i(TAG, "latency stage=texture_updated composition_us=${(System.nanoTime() - texture.timestamp) / 1000}")
                    }
                }
            }
        } else {
            val direct = original
            // Default surface ordering is behind the app window. The window's
            // transparent video hole leaves cursor/status views above the video.
            direct.setZOrderOnTop(false)
            direct.setZOrderMediaOverlay(false)
            surfaceView = direct
            videoView = direct
            direct.holder.addCallback(object : SurfaceHolder.Callback {
                override fun surfaceCreated(holder: SurfaceHolder) {
                    attachDecoder(holder.surface, false)
                }
                override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) = Unit
                override fun surfaceDestroyed(holder: SurfaceHolder) { stopDecoder() }
            })
        }
        val root = videoView.parent as FrameLayout
        root.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> layoutVideo() }
        cursorOverlay = CursorOverlay(videoView, findViewById<ImageView>(R.id.cursor))
        Log.i(TAG, "video output=${if (surfaceView != null) "surface" else "texture"}")
    }

    private fun layoutVideo() {
        if (!::videoView.isInitialized || streamWidth <= 0) return
        val parent = videoView.parent as FrameLayout
        val (width, height) = fitVideo(parent.width, parent.height, streamWidth, streamHeight)
        val params = videoView.layoutParams as FrameLayout.LayoutParams
        if (params.width != width || params.height != height || params.gravity != android.view.Gravity.CENTER) {
            params.width = width; params.height = height; params.gravity = android.view.Gravity.CENTER
            videoView.layoutParams = params
        }
    }

    private fun attachDecoder(surface: Surface, ownsSurface: Boolean) {
        stopDecoder()
        decoderSurface = surface
        ownsDecoderSurface = ownsSurface
        startConnection()
    }

    private fun requestFrameRate() {
        // A TextureView surface is consumed by the UI, so this hint only affects
        // direct display surfaces. Never modify the tablet's global settings.
        if (surfaceView != null) {
            decoderSurface?.takeIf { it.isValid }?.let { surface ->
                runCatching {
                    surface.setFrameRate(streamFramerate.toFloat(), Surface.FRAME_RATE_COMPATIBILITY_DEFAULT)
                }.onFailure { Log.w(TAG, "could not request stream frame rate", it) }
            }
        }
    }

    private fun stopDecoder() {
        val wasStreaming = videoSession != null
        videoSession = null
        decoder?.stop()
        decoder = null
        if (ownsDecoderSurface) decoderSurface?.release()
        decoderSurface = null
        ownsDecoderSurface = false
        if (wasStreaming) connection?.fail(getString(R.string.surface_replaced))
    }

    private fun createDecoder(surface: Surface): VideoDecoder {
        // A delayed codec failure belongs to its original connection.
        val manager = connection
        return VideoDecoder(surface, intent.getBooleanExtra("latency_trace", false),
            preferences.getBoolean("device_processing", false),
            onFailure = { reason -> manager?.fail(reason) })
    }

    private fun setupLobby() {
        findViewById<TextView>(R.id.device_details).text =
            getString(R.string.device_details, android.os.Build.MANUFACTURER, android.os.Build.MODEL,
                DeviceInfo.width, DeviceInfo.height, DeviceInfo.refreshRate.toInt(), DeviceInfo.deviceId)
        val spinner = findViewById<Spinner>(R.id.connection_method)
        spinner.adapter = ArrayAdapter(this, android.R.layout.simple_spinner_dropdown_item,
            resources.getStringArray(R.array.connection_methods).toList())
        val modes = listOf("auto", "adb", "accessory")
        spinner.setSelection(modes.indexOf(preferences.getString("transport", "auto")).coerceAtLeast(0))
        spinner.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onNothingSelected(parent: AdapterView<*>?) = Unit
            override fun onItemSelected(parent: AdapterView<*>?, view: View?, position: Int, id: Long) {
                val mode = modes[position]
                if (mode == preferences.getString("transport", "auto")) return
                intent.removeExtra("connection_transport")
                preferences.edit().putString("transport", mode).apply()
                retryConnection()
            }
        }
        findViewById<SwitchCompat>(R.id.device_processing).apply {
            isChecked = preferences.getBoolean("device_processing", false)
            setOnCheckedChangeListener { _, enabled ->
                preferences.edit().putBoolean("device_processing", enabled).apply()
                decoder?.selectNewestFrame = enabled
            }
        }
        findViewById<SwitchCompat>(R.id.local_ink).apply {
            localInk = preferences.getBoolean("local_ink", true)
            isChecked = localInk
            setOnCheckedChangeListener { _, enabled ->
                preferences.edit().putBoolean("local_ink", enabled).apply()
                localInk = enabled
                if (!enabled) ink.clear()
            }
        }
        findViewById<Button>(R.id.retry).setOnClickListener { retryConnection() }
        findViewById<Button>(R.id.resume).setOnClickListener {
            lobby.visibility = View.GONE
        }
        findViewById<Button>(R.id.display_test).setOnClickListener {
            AlertDialog.Builder(this).setTitle(R.string.display_test)
                .setView(DisplayCheckView(this)).setPositiveButton(R.string.close, null)
                .create().also { dialog ->
                    dialog.show()
                    dialog.window?.setLayout(WindowManager.LayoutParams.MATCH_PARENT,
                        WindowManager.LayoutParams.MATCH_PARENT)
                }
        }
    }
    private fun releaseTouches() {
        touches.cancel().forEach { connection?.sendTouch(it) }
        stylus.cancel().forEach { connection?.sendStylus(it) }
    }
    private fun showLobby() {
        ink.clear()
        releaseTouches()
        lobby.visibility = View.VISIBLE
        findViewById<Button>(R.id.resume).visibility = if (streamWidth > 0) View.VISIBLE else View.GONE
    }
    private fun resetConnection() {
        connectionEpoch++
        videoSession = null
        touches.cancel()
        stylus.cancel()
        main.removeCallbacks(reconnect)
        main.removeCallbacks(statsTicker)
        connection?.close(); connection = null
        pendingAccessory?.close(); pendingAccessory = null
        camera?.stop(); camera = null; waitingCamera = null
        decoder?.stop()
        streamWidth = 0; streamHeight = 0
        accessoryConnected = false
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (::cursorOverlay.isInitialized) {
            cursorOverlay.hide()
        }
        showLobby()
    }
    private val reconnect = Runnable {
        if (!destroyed) { resetConnection(); accessories.reset(); startConnection(); accessories.check() }
    }
    private fun retryConnection() {
        resetConnection()
        accessories.reset()
        startConnection()
        accessories.check()
    }
    private fun startConnection() {
        if (connection != null || decoderSurface == null || destroyed) return
        val fd = pendingAccessory
        pendingAccessory = null
        showStatus(getString(R.string.waiting_for_host))
        val epoch = connectionEpoch
        val owner = this
        val callbacks = object : ConnectionManager.Callbacks {
            override fun onVideoConfig(width: Int, height: Int, framerate: Int) {
                onVideoConfig(width, height, framerate, "h264")
            }
            override fun onVideoConfig(width: Int, height: Int, framerate: Int, codec: String) {
                // Retain the first IDR on the reader while the main thread sets
                // up and publishes the decoder for this connection.
                val task = FutureTask<Unit> {
                    if (connectionEpoch == epoch && !destroyed) owner.onVideoConfig(width, height, framerate, codec)
                }
                main.post(task)
                try { task.get(5, TimeUnit.SECONDS) }
                catch (e: Exception) { task.cancel(false); throw e }
            }
            override fun onVideoFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean) {
                owner.decodeVideoFrame(epoch, data, length, ptsUs, isConfig)
            }
            override fun onCameraControl(enabled: Boolean, cameraId: String, width: Int, height: Int, framerate: Int, bitrateKbps: Int, generation: Long) {
                main.post {
                    if (connectionEpoch == epoch && !destroyed) owner.onCameraControl(enabled, cameraId, width, height, framerate, bitrateKbps, generation)
                }
            }
            override fun onCursor(update: CursorUpdate) {
                main.post { if (connectionEpoch == epoch && !destroyed) owner.onCursor(update) }
            }
            override fun onOpenSettings() {
                main.post { if (connectionEpoch == epoch && !destroyed) owner.onOpenSettings() }
            }
            override fun onConnected() {
                main.post { if (connectionEpoch == epoch && !destroyed) owner.onConnected() }
            }
            override fun onDisconnected(reason: String) {
                main.post { if (connectionEpoch == epoch && !destroyed) owner.onDisconnected(reason) }
            }
        }
        runCatching {
            ConnectionManager(callbacks, fd,
                TransportMode.parse(preferences.getString("transport", "auto") ?: "auto"),
                traceLatency = intent.getBooleanExtra("latency_trace", false))
                .also { connection = it; it.start() }
        }.onFailure {
            connection?.close(); connection = null
            fd?.close(); showStatus(it.message ?: getString(R.string.listen_failed))
        }
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        accessories.check()
    }
    override fun onResume() {
        super.onResume()
        if (::accessories.isInitialized) accessories.check()
    }

    private fun goFullscreen() {
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    /** Safe to call from any thread; hops to the main thread itself. */
    private fun showStatus(text: String?) {
        val update = {
            statusView.text = text ?: ""
            statusView.visibility = if (text == null) View.GONE else View.VISIBLE
            if (text != null) showLobby()
        }
        if (Looper.myLooper() == Looper.getMainLooper()) update() else main.post { update() }
    }

    // ------------------------------------------------------- host callbacks
    // The per-connection adapter serializes UI callbacks on the main thread and
    // rejects callbacks from obsolete sessions. Video inputs stay on the reader.
    override fun onVideoConfig(width: Int, height: Int, framerate: Int) {
        onVideoConfig(width, height, framerate, "h264")
    }

    override fun onVideoConfig(width: Int, height: Int, framerate: Int, codec: String) {
        require(width > 0 && height > 0 && framerate > 0) { "Invalid video configuration" }
        videoSession = null
        streamWidth = width
        streamHeight = height
        streamFramerate = framerate
        textureView?.surfaceTexture?.setDefaultBufferSize(width, height)
        surfaceView?.holder?.setFixedSize(width, height)
        requestFrameRate()
        layoutVideo()
        cursorOverlay.setStreamSize(width, height)
        decoder?.stop()
        val surface = decoderSurface ?: throw ProtocolException("Video surface is unavailable")
        val target = createDecoder(surface)
        decoder = target
        target.start(width, height, null, codec)
        videoSession = VideoSession(connectionEpoch, target)
        main.removeCallbacks(statsTicker)
        main.postDelayed(statsTicker, STATS_INTERVAL_MS)
        statusView.text = getString(R.string.streaming_status, width, height, framerate,
            getString(if (accessoryConnected) R.string.transport_accessory else R.string.transport_adb))
        statusView.visibility = View.VISIBLE
        lobby.visibility = View.GONE
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        Log.i(TAG, "stream configured ${width}x$height @$framerate codec=$codec")
    }

    override fun onVideoFrame(data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean) {
        decodeVideoFrame(connectionEpoch, data, length, ptsUs, isConfig)
    }

    private fun decodeVideoFrame(epoch: Int, data: ByteArray, length: Int, ptsUs: Long, isConfig: Boolean) {
        if (epoch != connectionEpoch) return
        val session = videoSession ?: throw ProtocolException("Video surface is unavailable; restart the stream")
        if (session.epoch != epoch) return
        session.decoder.decode(data, length, ptsUs, isConfig)
    }

    override fun onCameraControl(
        enabled: Boolean, cameraId: String, width: Int, height: Int, framerate: Int, bitrateKbps: Int, generation: Long,
    ) {
        camera?.stop(); camera = null; waitingCamera = null
        val manager = connection
        val epoch = connectionEpoch
        if (!enabled) { manager?.sendCameraStatus("off", getString(R.string.camera_off), generation); return }
        manager?.sendCameraStatus("pending", getString(R.string.camera_starting), generation)
        val start = {
            if (epoch == connectionEpoch && !destroyed) {
                camera = CameraSource(this,
                    onStatus = { state, message -> if (epoch == connectionEpoch) manager?.sendCameraStatus(state, message, generation) },
                    onFrame = { data, length, ptsUs, isConfig, isKey ->
                        if (epoch == connectionEpoch) manager?.sendCameraFrame(data, length, ptsUs, isConfig, isKey, generation)
                    }).also { it.start(cameraId, width, height, framerate, bitrateKbps) }
            }
        }
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) start()
        else {
            manager?.sendCameraStatus("pending", getString(R.string.camera_permission_pending), generation)
            waitingCamera = start; cameraPermission.launch(Manifest.permission.CAMERA)
        }
    }

    override fun onCursor(update: CursorUpdate) {
        cursorOverlay.submit(update)
    }

    override fun onOpenSettings() {
        showLobby()
    }

    override fun onConnected() {
        showStatus(getString(R.string.connected_waiting))
    }

    override fun onDisconnected(reason: String) {
        Log.w(TAG, "disconnected: $reason")
        if (destroyed) return
        val wasAccessory = accessoryConnected
        resetConnection()
        if (!wasAccessory) intent.removeExtra("connection_transport")
        showStatus(getString(R.string.disconnected, reason))
        // Existing accessory consent survives host restarts until the cable is detached.
        if (!reason.startsWith("Incompatible connection methods:")) main.postDelayed(reconnect, 1500)
    }

    // --------------------------------------------------- stylus & multitouch
    /** Device-clock time of the sample being sent; travels in the frame header. */
    private var inputEventUs = 0L

    private fun eventUs(e: MotionEvent): Long =
        if (Build.VERSION.SDK_INT >= 34) e.eventTimeNanos / 1000 else e.eventTime * 1000

    private fun historicalEventUs(e: MotionEvent, h: Int): Long =
        if (Build.VERSION.SDK_INT >= 34) e.getHistoricalEventTimeNanos(h) / 1000 else e.getHistoricalEventTime(h) * 1000

    private fun isStylusTool(toolType: Int): Boolean =
        toolType == MotionEvent.TOOL_TYPE_STYLUS || toolType == MotionEvent.TOOL_TYPE_ERASER

    private fun extractStylusButtons(buttonState: Int): Int {
        var mask = 0
        if (buttonState and (MotionEvent.BUTTON_STYLUS_PRIMARY or MotionEvent.BUTTON_SECONDARY) != 0) {
            mask = mask or Protocol.StylusButtons.PRIMARY
        }
        if (buttonState and (MotionEvent.BUTTON_STYLUS_SECONDARY or MotionEvent.BUTTON_TERTIARY) != 0) {
            mask = mask or Protocol.StylusButtons.SECONDARY
        }
        return mask
    }

    private fun extractStylusTilt(tiltRad: Float, orientationRad: Float): Pair<Float, Float> {
        val normalizedTilt = (tiltRad / (Math.PI.toFloat() / 2f)).coerceIn(0f, 1f)
        val tx = (kotlin.math.sin(orientationRad.toDouble()) * normalizedTilt).toFloat().coerceIn(-1f, 1f)
        val ty = (-kotlin.math.cos(orientationRad.toDouble()) * normalizedTilt).toFloat().coerceIn(-1f, 1f)
        return tx to ty
    }

    override fun onGenericMotionEvent(event: MotionEvent): Boolean {
        if (lobby.visibility == View.VISIBLE) return super.onGenericMotionEvent(event)
        val conn = connection ?: return super.onGenericMotionEvent(event)
        if (streamWidth == 0 || streamHeight == 0) return super.onGenericMotionEvent(event)
        if (event.pointerCount == 0 || !isStylusTool(event.getToolType(0))) {
            return super.onGenericMotionEvent(event)
        }
        inputEventUs = eventUs(event)

        val tool = if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
            Protocol.StylusTool.ERASER
        } else {
            Protocol.StylusTool.PEN
        }

        when (event.actionMasked) {
            MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_MOVE -> {
                // Track proximity for palm rejection. The host consumes hover
                // only to release contact; it never moves the desktop pointer.
                touches.cancel().forEach { conn.sendTouch(it) }
                sendTrackedStylus(
                    conn,
                    StylusEvent(Protocol.StylusAction.HOVER, tool, 0, 0.0, 0.0, 0f, 0f, 0f),
                )
                return true
            }
            MotionEvent.ACTION_HOVER_EXIT -> {
                sendTrackedStylus(
                    conn,
                    StylusEvent(Protocol.StylusAction.HOVER_EXIT, tool, 0, 0.0, 0.0, 0f, 0f, 0f),
                )
                return true
            }
        }
        return super.onGenericMotionEvent(event)
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_CANCEL) {
            releaseTouches()
            return true
        }
        if (lobby.visibility == View.VISIBLE) return super.onTouchEvent(event)
        val conn = connection ?: return false
        if (streamWidth == 0 || streamHeight == 0) return false
        inputEventUs = eventUs(event)

        // The surface may be letterboxed if the host's monitor aspect ratio does
        // not exactly match the panel, so map through the displayed rectangle
        // rather than assuming the view fills the screen.
        val viewW = videoView.width.toFloat()
        val viewH = videoView.height.toFloat()
        if (viewW <= 0f || viewH <= 0f) return false
        val scale = minOf(viewW / streamWidth, viewH / streamHeight)
        val offsetX = (viewW - streamWidth * scale) / 2f
        val offsetY = (viewH - streamHeight * scale) / 2f

        fun mapX(raw: Float) = ((raw - videoView.left - offsetX) / scale).toDouble().coerceIn(0.0, streamWidth - 1.0)
        fun mapY(raw: Float) = ((raw - videoView.top - offsetY) / scale).toDouble().coerceIn(0.0, streamHeight - 1.0)

        val actionIdx = if (event.actionMasked == MotionEvent.ACTION_MOVE) {
            // MOVE has no changed pointer: actionIndex is always zero, which
            // may be a palm that entered before the pen.
            (0 until event.pointerCount).firstOrNull { isStylusTool(event.getToolType(it)) } ?: event.actionIndex
        } else {
            event.actionIndex
        }
        if (isStylusTool(event.getToolType(actionIdx))) {
            // Active stylus rejects simultaneous finger/palm touches.
            touches.cancel().forEach { conn.sendTouch(it) }
            val tool = if (event.getToolType(actionIdx) == MotionEvent.TOOL_TYPE_ERASER) {
                Protocol.StylusTool.ERASER
            } else {
                Protocol.StylusTool.PEN
            }
            val buttons = extractStylusButtons(event.buttonState)
            fun sample(action: Byte, history: Int? = null) {
                inputEventUs = history?.let { historicalEventUs(event, it) } ?: eventUs(event)
                val x = history?.let { event.getHistoricalX(actionIdx, it) } ?: event.getX(actionIdx)
                val y = history?.let { event.getHistoricalY(actionIdx, it) } ?: event.getY(actionIdx)
                val pressure = if (action == Protocol.StylusAction.UP) 0f else {
                    (history?.let { event.getHistoricalPressure(actionIdx, it) }
                        ?: event.getPressure(actionIdx)).coerceIn(0.01f, 1f)
                }
                fun axis(axis: Int) = history?.let { event.getHistoricalAxisValue(axis, actionIdx, it) }
                    ?: event.getAxisValue(axis, actionIdx)
                val (tx, ty) = extractStylusTilt(axis(MotionEvent.AXIS_TILT), axis(MotionEvent.AXIS_ORIENTATION))
                if (localInk && tool == Protocol.StylusTool.PEN) {
                    if (action == Protocol.StylusAction.UP) ink.lift()
                    else ink.add(x, y, pressure, start = action == Protocol.StylusAction.DOWN, atNs = inputEventUs * 1000)
                }
                sendTrackedStylus(conn, StylusEvent(action, tool, buttons, mapX(x), mapY(y), pressure, tx, ty))
            }
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                    if (event.getX(actionIdx) < videoView.left || event.getX(actionIdx) >= videoView.right ||
                        event.getY(actionIdx) < videoView.top || event.getY(actionIdx) >= videoView.bottom) return true
                    sample(Protocol.StylusAction.DOWN)
                }
                MotionEvent.ACTION_MOVE -> {
                    for (h in 0 until event.historySize) sample(Protocol.StylusAction.MOTION, h)
                    sample(Protocol.StylusAction.MOTION)
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> sample(Protocol.StylusAction.UP)
            }
            return true
        }

        // Palm rejection: ignore finger touches while the stylus is hovering or drawing.
        if (stylus.isActive) return true

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val i = event.actionIndex
                // A tap on the black bars must not become a click on a desktop edge.
                if (event.getX(i) < videoView.left || event.getX(i) >= videoView.right ||
                    event.getY(i) < videoView.top || event.getY(i) >= videoView.bottom) return true
                sendTrackedTouch(conn,
                    TouchEvent(
                        Protocol.TouchAction.DOWN, event.getPointerId(i),
                        mapX(event.getX(i)), mapY(event.getY(i)),
                    )
                )
            }
            MotionEvent.ACTION_MOVE -> {
                // Emit batched historical samples and the current sample for each finger that moved.
                for (h in 0 until event.historySize) {
                    inputEventUs = historicalEventUs(event, h)
                    for (i in 0 until event.pointerCount) {
                        if (isStylusTool(event.getToolType(i))) continue
                        sendTrackedTouch(conn,
                            TouchEvent(
                                Protocol.TouchAction.MOTION, event.getPointerId(i),
                                mapX(event.getHistoricalX(i, h)), mapY(event.getHistoricalY(i, h)),
                            )
                        )
                    }
                }
                inputEventUs = eventUs(event)
                for (i in 0 until event.pointerCount) {
                    if (isStylusTool(event.getToolType(i))) continue
                    sendTrackedTouch(conn,
                        TouchEvent(
                            Protocol.TouchAction.MOTION, event.getPointerId(i),
                            mapX(event.getX(i)), mapY(event.getY(i)),
                        )
                    )
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                val i = event.actionIndex
                sendTrackedTouch(conn,
                    TouchEvent(
                        Protocol.TouchAction.UP, event.getPointerId(i),
                        mapX(event.getX(i)), mapY(event.getY(i)),
                    )
                )
            }
        }
        return true
    }

    private fun sendTrackedTouch(conn: ConnectionManager, event: TouchEvent) {
        touches.record(event)?.let { conn.sendTouch(it, inputEventUs) }
    }

    private fun sendTrackedStylus(conn: ConnectionManager, event: StylusEvent) {
        stylus.record(event)?.let { conn.sendStylus(it, inputEventUs) }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) goFullscreen() else releaseTouches()
    }

    override fun onDestroy() {
        destroyed = true
        main.removeCallbacks(reconnect)
        main.removeCallbacks(statsTicker)
        accessories.close()
        pendingAccessory?.close()
        camera?.stop()
        connection?.close()
        stopDecoder()
        cursorOverlay.detach()
        super.onDestroy()
    }

    private companion object {
        const val TAG = "extraspace"
        const val STATS_INTERVAL_MS = 500L
    }
}
