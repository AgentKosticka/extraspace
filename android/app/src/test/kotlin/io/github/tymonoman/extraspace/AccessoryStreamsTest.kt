package io.github.tymonoman.extraspace

import java.io.InputStream
import java.io.OutputStream
import java.io.ByteArrayOutputStream
import org.junit.Assert.*
import org.junit.Test

class AccessoryStreamsTest {
    @Test fun fragmentedTransfersPreserveHeadersAndLargePayloads() {
        val bytes = ByteArrayOutputStream()
        val writer = FrameWriter(bytes)
        val video = ByteArray(38000) { (it % 251).toByte() }
        writer.write(Protocol.Channel.VIDEO_DOWN, 0, Protocol.Flags.KEYFRAME, 123, video)
        writer.write(Protocol.Channel.CONTROL, Protocol.ControlKind.PING, 0, 456, ByteArray(0))
        val data = bytes.toByteArray()
        val transfers = object : InputStream() {
            var position = 0
            override fun read(): Int = error("USB reads must use a full transfer buffer")
            override fun read(dest: ByteArray, offset: Int, length: Int): Int {
                assertTrue(length >= 16384)
                if (position == data.size) return -1
                val count = minOf(16384, data.size - position)
                data.copyInto(dest, offset, position, position + count)
                position += count
                return count
            }
        }
        val reader = FrameReader(AccessoryInput(transfers))
        val head = reader.readHeader()
        assertEquals(38000, head.length); assertEquals(123L, head.ptsUs)
        assertArrayEquals(video, reader.readPayload(head))
        assertEquals(Protocol.ControlKind.PING, reader.readHeader().kind)
    }
    @Test fun outputAlwaysFitsUsbTransferLimits() {
        val bytes = ByteArrayOutputStream()
        val output = object : OutputStream() {
            override fun write(value: Int) = error("expected a bulk write")
            override fun write(source: ByteArray, offset: Int, length: Int) {
                assertTrue(length <= 16384)
                bytes.write(source, offset, length)
            }
        }
        val payload = ByteArray(72000) { it.toByte() }
        AccessoryOutput(output).write(payload)
        assertArrayEquals(payload, bytes.toByteArray())
    }
}
