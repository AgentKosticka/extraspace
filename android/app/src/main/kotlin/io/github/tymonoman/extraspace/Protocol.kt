package io.github.tymonoman.extraspace

import java.io.DataInputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Wire protocol, mirroring `xs-proto` on the host.
 *
 * Both sides consume shared golden vectors under `protocol/` in CI. Update both
 * implementations and the vectors for wire changes; magic alone cannot detect
 * layout or message-kind drift.
 */
object Protocol {
    /** Reads as the ASCII bytes `XSPA` little-endian. */
    const val MAGIC: Int = 0x41505358
    const val HEADER_LEN: Int = 20
    const val MAX_PAYLOAD: Int = 16 * 1024 * 1024
    const val VERSION: Int = 2

    object Channel {
        const val CONTROL: Byte = 0
        const val TOUCH: Byte = 1
        const val VIDEO_DOWN: Byte = 2
        const val CAMERA_UP: Byte = 3
    }

    object Flags {
        const val KEYFRAME: Short = 1
        const val CODEC_CONFIG: Short = 2
        /** More of this video frame follows; only the USB accessory bridge splits frames. */
        const val FRAGMENT: Short = 4
    }

    object ControlKind {
        const val HELLO: Byte = 0
        const val VIDEO_CONFIG: Byte = 1
        const val STATS: Byte = 2
        const val CAMERA_CONTROL: Byte = 3
        const val PING: Byte = 4
        const val PONG: Byte = 5
        const val ERROR: Byte = 6
        const val CURSOR: Byte = 7
        const val HELLO_REQUEST: Byte = 8
        const val SESSION_END: Byte = 9
        const val CAMERA_STATUS: Byte = 10
        const val OPEN_SETTINGS: Byte = 11
    }

    object CursorFlags {
        const val VISIBLE: Int = 1
        const val POSITION: Int = 2
        const val HOTSPOT: Int = 4
        const val BITMAP: Int = 8
    }

    object TouchKind {
        const val FINGER: Byte = 0
        const val STYLUS: Byte = 1
    }

    object TouchAction {
        const val DOWN: Byte = 0
        const val MOTION: Byte = 1
        const val UP: Byte = 2
    }

    object StylusAction {
        const val HOVER: Byte = 0
        const val DOWN: Byte = 1
        const val MOTION: Byte = 2
        const val UP: Byte = 3
        const val HOVER_EXIT: Byte = 4
    }

    object StylusTool {
        const val PEN: Byte = 0
        const val ERASER: Byte = 1
    }

    object StylusButtons {
        const val PRIMARY: Int = 1
        const val SECONDARY: Int = 2
        const val TERTIARY: Int = 4
    }

    /** Abstract socket names; must match `sockets` in `xs-transport`. */
    object Sockets {
        const val CONTROL = "extraspace-control"
        const val VIDEO = "extraspace-video"
        const val CAMERA = "extraspace-camera"
    }
}

data class FrameHeader(
    val channel: Byte,
    val kind: Byte,
    val flags: Short,
    val length: Int,
    val ptsUs: Long,
)

class ProtocolException(message: String) : IOException(message)

/** Reads length-prefixed frames from a stream. Not thread-safe; one per socket. */
class FrameReader(input: InputStream) {
    private val stream = DataInputStream(input.buffered(64 * 1024))
    private val headerBuf = ByteArray(Protocol.HEADER_LEN)

    fun readHeader(): FrameHeader {
        stream.readFully(headerBuf)
        val bb = ByteBuffer.wrap(headerBuf).order(ByteOrder.LITTLE_ENDIAN)
        val magic = bb.int
        if (magic != Protocol.MAGIC) {
            throw ProtocolException(
                "bad magic 0x${magic.toUInt().toString(16)} - host/app protocol mismatch " +
                    "or a desynced stream"
            )
        }
        val channel = bb.get()
        val kind = bb.get()
        val flags = bb.short
        val length = bb.int
        val ptsUs = bb.long
        if (length < 0 || length > Protocol.MAX_PAYLOAD) {
            throw ProtocolException("payload of $length bytes is out of range")
        }
        return FrameHeader(channel, kind, flags, length, ptsUs)
    }

    /** Reads a payload into [dest] at [offset]; it must have [FrameHeader.length] bytes free there. */
    fun readPayload(header: FrameHeader, dest: ByteArray, offset: Int = 0) {
        if (header.length > 0) stream.readFully(dest, offset, header.length)
    }

    fun readPayload(header: FrameHeader): ByteArray {
        val buf = ByteArray(header.length)
        readPayload(header, buf)
        return buf
    }
}

/**
 * Rebuilds video frames that the host's USB accessory bridge split with
 * [Protocol.Flags.FRAGMENT], so control frames could travel between pieces.
 * Unsplit frames pass straight through. Not thread-safe; one per reader.
 */
class VideoAssembler(initialCapacity: Int = 512 * 1024) {
    var buffer = ByteArray(initialCapacity)
        private set
    private var size = 0

    /** Reads one piece; returns the whole frame's length on its last piece, else -1. */
    fun read(reader: FrameReader, header: FrameHeader): Int {
        val total = size + header.length
        if (total > Protocol.MAX_PAYLOAD) throw ProtocolException("video frame of $total bytes is out of range")
        if (total > buffer.size) buffer = buffer.copyOf(maxOf(total, minOf(buffer.size * 2, Protocol.MAX_PAYLOAD)))
        reader.readPayload(header, buffer, size)
        if (header.flags.toInt() and Protocol.Flags.FRAGMENT.toInt() != 0) {
            size = total
            return -1
        }
        size = 0
        return total
    }
}

/** Writes frames to a stream. Synchronised: several producers share one socket. */
class FrameWriter(output: OutputStream) {
    private val stream = output.buffered(64 * 1024)
    private val headerBuf = ByteArray(Protocol.HEADER_LEN)

    @Synchronized
    fun write(channel: Byte, kind: Byte, flags: Short, ptsUs: Long, payload: ByteArray, length: Int = payload.size) {
        ByteBuffer.wrap(headerBuf).order(ByteOrder.LITTLE_ENDIAN).apply {
            putInt(Protocol.MAGIC)
            put(channel)
            put(kind)
            putShort(flags)
            putInt(length)
            putLong(ptsUs)
        }
        stream.write(headerBuf)
        if (length > 0) stream.write(payload, 0, length)
        stream.flush()
    }
}

/**
 * A single touch point. Fixed 21-byte encoding rather than JSON: these arrive at
 * up to 120 Hz per finger and parse cost is not free.
 */
/**
 * Host -> device cursor overlay. Binary, matching `CursorMessage` in xs-proto.
 *
 * Position-only updates reuse the last sprite. A hide is a single zero flags byte.
 */
data class CursorUpdate(
    val visible: Boolean,
    val hasPosition: Boolean,
    val x: Int,
    val y: Int,
    val hasHotspot: Boolean,
    val hotX: Int,
    val hotY: Int,
    val bitmap: ByteArray?,
    val bitmapWidth: Int,
    val bitmapHeight: Int,
) {
    companion object {
        fun hide(): CursorUpdate = CursorUpdate(
            visible = false,
            hasPosition = false,
            x = 0,
            y = 0,
            hasHotspot = false,
            hotX = 0,
            hotY = 0,
            bitmap = null,
            bitmapWidth = 0,
            bitmapHeight = 0,
        )

        fun decode(payload: ByteArray): CursorUpdate? {
            if (payload.isEmpty()) return null
            val bb = ByteBuffer.wrap(payload).order(ByteOrder.LITTLE_ENDIAN)
            val flags = bb.get().toInt() and 0xff
            var hasPosition = false
            var x = 0
            var y = 0
            if (flags and Protocol.CursorFlags.POSITION != 0) {
                if (bb.remaining() < 8) return null
                x = bb.int
                y = bb.int
                hasPosition = true
            }
            var hasHotspot = false
            var hotX = 0
            var hotY = 0
            if (flags and Protocol.CursorFlags.HOTSPOT != 0) {
                if (bb.remaining() < 4) return null
                hotX = bb.short.toInt()
                hotY = bb.short.toInt()
                hasHotspot = true
            }
            var bitmap: ByteArray? = null
            var bitmapWidth = 0
            var bitmapHeight = 0
            if (flags and Protocol.CursorFlags.BITMAP != 0) {
                if (bb.remaining() < 4) return null
                bitmapWidth = bb.short.toInt() and 0xffff
                bitmapHeight = bb.short.toInt() and 0xffff
                val pixels = bitmapWidth * bitmapHeight * 4
                if (pixels < 0 || bb.remaining() != pixels) return null
                bitmap = ByteArray(pixels)
                bb.get(bitmap)
            } else if (bb.remaining() != 0) {
                return null
            }
            return CursorUpdate(
                visible = flags and Protocol.CursorFlags.VISIBLE != 0,
                hasPosition = hasPosition,
                x = x,
                y = y,
                hasHotspot = hasHotspot,
                hotX = hotX,
                hotY = hotY,
                bitmap = bitmap,
                bitmapWidth = bitmapWidth,
                bitmapHeight = bitmapHeight,
            )
        }
    }
}

data class TouchEvent(val action: Byte, val slot: Int, val x: Double, val y: Double) {
    fun encode(): ByteArray =
        ByteBuffer.allocate(TOUCH_PAYLOAD_LEN).order(ByteOrder.LITTLE_ENDIAN).apply {
            put(action)
            putInt(slot)
            putDouble(x)
            putDouble(y)
        }.array()

    companion object {
        const val TOUCH_PAYLOAD_LEN = 21
    }
}

data class StylusEvent(
    val action: Byte,
    val tool: Byte,
    val buttons: Int,
    val x: Double,
    val y: Double,
    val pressure: Float,
    val tiltX: Float,
    val tiltY: Float,
) {
    fun encode(): ByteArray =
        ByteBuffer.allocate(STYLUS_PAYLOAD_LEN).order(ByteOrder.LITTLE_ENDIAN).apply {
            put(action)
            put(tool)
            put((buttons and 0xff).toByte())
            put(0)
            putDouble(x)
            putDouble(y)
            putFloat(pressure.coerceIn(0f, 1f))
            putFloat(tiltX.coerceIn(-1f, 1f))
            putFloat(tiltY.coerceIn(-1f, 1f))
        }.array()

    companion object {
        const val STYLUS_PAYLOAD_LEN = 32
    }
}
