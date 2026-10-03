package io.github.tymonoman.extraspace

import org.junit.Assert.*
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream

class ProtocolGoldenTest {
    private fun hex(value: String): ByteArray = value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    @Test fun sharedWireVectorsMatchBothParsingAndSerialization() {
        val kinds = listOf(Protocol.ControlKind.HELLO, Protocol.ControlKind.VIDEO_CONFIG,
            Protocol.ControlKind.STATS, Protocol.ControlKind.CAMERA_CONTROL, Protocol.ControlKind.PING,
            Protocol.ControlKind.PONG, Protocol.ControlKind.ERROR, Protocol.ControlKind.CURSOR,
            Protocol.ControlKind.HELLO_REQUEST, Protocol.ControlKind.SESSION_END, Protocol.ControlKind.CAMERA_STATUS)
        assertEquals(listOf<Byte>(0, 1, 2, 3), listOf(Protocol.Channel.CONTROL, Protocol.Channel.TOUCH, Protocol.Channel.VIDEO_DOWN, Protocol.Channel.CAMERA_UP))
        assertEquals(listOf<Short>(1, 2), listOf(Protocol.Flags.KEYFRAME, Protocol.Flags.CODEC_CONFIG))
        assertEquals(listOf<Byte>(0, 1, 2), listOf(Protocol.TouchAction.DOWN, Protocol.TouchAction.MOTION, Protocol.TouchAction.UP))
        assertEquals(listOf(1, 2, 4, 8), listOf(Protocol.CursorFlags.VISIBLE, Protocol.CursorFlags.POSITION, Protocol.CursorFlags.HOTSPOT, Protocol.CursorFlags.BITMAP))
        assertEquals(1, Protocol.VERSION)
        val vectors = javaClass.getResourceAsStream("/golden-vectors.tsv")!!.bufferedReader().readLines()
        for (line in vectors.filter { !it.startsWith("#") && it.isNotBlank() }) {
            val f = line.split('\t')
            val bytes = hex(f[2])
            when (f[0]) {
                "header" -> {
                    val h = FrameReader(ByteArrayInputStream(bytes)).readHeader()
                    assertEquals(f[3].toByte(), h.channel)
                    assertEquals(f[4].toByte(), h.kind)
                    assertEquals(f[5].toShort(), h.flags)
                    assertEquals(f[6].toInt(), h.length)
                    assertEquals(f[7].toLong(), h.ptsUs)
                    if (h.channel == Protocol.Channel.CONTROL) assertEquals(f[4].toByte(), kinds[f[4].toInt()])
                    val out = ByteArrayOutputStream()
                    FrameWriter(out).write(h.channel, h.kind, h.flags, h.ptsUs, ByteArray(h.length))
                    assertArrayEquals(f[1], bytes, out.toByteArray().copyOf(Protocol.HEADER_LEN))
                }
                "touch" -> assertArrayEquals(f[1], bytes, TouchEvent(f[3].toByte(), f[4].toInt(), f[5].toDouble(), f[6].toDouble()).encode())
                "cursor" -> {
                    val c = CursorUpdate.decode(bytes)!!
                    assertEquals(f[3].toBoolean(), c.visible)
                    assertEquals(f[4] != "-", c.hasPosition)
                    if (c.hasPosition) { assertEquals(f[4].toInt(), c.x); assertEquals(f[5].toInt(), c.y) }
                    assertEquals(f[6] != "-", c.hasHotspot)
                    if (c.hasHotspot) { assertEquals(f[6].toInt(), c.hotX); assertEquals(f[7].toInt(), c.hotY) }
                    if (f[8] == "-") assertNull(c.bitmap)
                    else {
                        assertEquals(f[8].toInt(), c.bitmapWidth); assertEquals(f[9].toInt(), c.bitmapHeight)
                        assertArrayEquals(hex(f[10]), c.bitmap)
                    }
                }
                else -> fail("Unknown vector ${f[0]}")
            }
        }
    }

    @Test fun malformedAndOversizedFramesFailBeforeAllocation() {
        val invalid = byteArrayOf(0, 0, 0, 0) + ByteArray(16)
        assertThrows(ProtocolException::class.java) { FrameReader(ByteArrayInputStream(invalid)).readHeader() }
        val oversized = hex("5853504102000000010000010000000000000000")
        assertThrows(ProtocolException::class.java) { FrameReader(ByteArrayInputStream(oversized)).readHeader() }
        val negative = hex("5853504102000000ffffffff0000000000000000")
        assertThrows(ProtocolException::class.java) { FrameReader(ByteArrayInputStream(negative)).readHeader() }
        assertNull(CursorUpdate.decode(byteArrayOf(15)))
        assertNull(CursorUpdate.decode(byteArrayOf(0, 1)))
    }
}
