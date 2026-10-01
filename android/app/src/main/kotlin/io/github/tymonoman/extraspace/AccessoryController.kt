package io.github.tymonoman.extraspace

import android.app.AlertDialog
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.os.ParcelFileDescriptor
import androidx.activity.ComponentActivity
import androidx.core.content.ContextCompat
import java.io.Closeable

/** Activity-scoped accessory discovery and user consent. No ADB dependency. */
class AccessoryController(
    private val activity: ComponentActivity,
    private val enabled: () -> Boolean,
    private val status: (String) -> Unit,
    private val ready: (ParcelFileDescriptor) -> Unit,
    private val detached: () -> Unit,
) : Closeable {
    private val manager = activity.getSystemService(UsbManager::class.java)
    private val permissionAction = "${activity.packageName}.USB_PERMISSION"
    private var pending: UsbAccessory? = null
    private var approved: UsbAccessory? = null
    private var connected: UsbAccessory? = null
    private var dialog: AlertDialog? = null
    private var denied = false
    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            @Suppress("DEPRECATION")
            val accessory = intent.getParcelableExtra<UsbAccessory>(UsbManager.EXTRA_ACCESSORY)
            when (intent.action) {
                permissionAction -> {
                    val expected = pending ?: return
                    if (accessory != expected) return
                    pending = null
                    // Verify the OS permission and current attachment, never trust broadcast extras.
                    if (intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                        && manager.hasPermission(expected)) open(expected)
                    else { denied = true; status(activity.getString(R.string.accessory_denied)) }
                }
                UsbManager.ACTION_USB_ACCESSORY_ATTACHED -> { denied = false; check() }
                UsbManager.ACTION_USB_ACCESSORY_DETACHED -> {
                    if (accessory == connected || accessory == pending) {
                        connected = null; pending = null; approved = null
                        dialog?.dismiss(); dialog = null
                        detached()
                    }
                }
            }
        }
    }
    init {
        ContextCompat.registerReceiver(activity, receiver, IntentFilter().apply {
            addAction(permissionAction)
            addAction(UsbManager.ACTION_USB_ACCESSORY_ATTACHED)
            addAction(UsbManager.ACTION_USB_ACCESSORY_DETACHED)
        }, ContextCompat.RECEIVER_NOT_EXPORTED)
    }
    fun reset() { connected = null; pending = null; denied = false; dialog?.dismiss(); dialog = null }
    fun check() {
        if (!enabled() || denied || pending != null || connected != null || dialog != null) return
        val accessory = manager.accessoryList?.firstOrNull {
            it.manufacturer == "ExtraSpace" && it.model == "ExtraSpace Display" && it.version == "1"
        } ?: return
        if (accessory == approved && manager.hasPermission(accessory)) { open(accessory); return }
        pending = accessory
        dialog = AlertDialog.Builder(activity)
            .setTitle(R.string.allow_accessory).setMessage(R.string.accessory_explanation)
            .setPositiveButton(R.string.allow) { _, _ ->
                dialog = null
                approved = accessory
                if (manager.hasPermission(accessory)) { pending = null; open(accessory) }
                else {
                    status(activity.getString(R.string.usb_permission_waiting))
                    manager.requestPermission(accessory, PendingIntent.getBroadcast(activity, 0,
                        Intent(permissionAction).setPackage(activity.packageName),
                        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE))
                }
            }
            .setNegativeButton(R.string.deny) { _, _ -> deny() }
            .setOnCancelListener { deny() }
            .create().also { it.show() }
    }
    private fun deny() {
        dialog = null; pending = null; approved = null; denied = true
        status(activity.getString(R.string.accessory_denied))
    }
    private fun open(accessory: UsbAccessory) {
        if (!enabled() || manager.accessoryList?.contains(accessory) != true || !manager.hasPermission(accessory)) return
        runCatching { manager.openAccessory(accessory) ?: error("Could not open USB accessory") }
            .onSuccess { fd -> connected = accessory; ready(fd) }
            .onFailure { status(it.message ?: "USB accessory unavailable") }
    }
    override fun close() {
        dialog?.dismiss(); dialog = null
        activity.unregisterReceiver(receiver)
    }
}
