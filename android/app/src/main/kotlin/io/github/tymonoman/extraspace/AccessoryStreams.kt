package io.github.tymonoman.extraspace

import java.io.InputStream
import java.io.OutputStream

/** Never read part of a USB transfer from the kernel: Android discards the rest.
 * Buffer complete 16 KiB transfers, then serve the framing parser's small reads. */
internal class AccessoryInput(private val input: InputStream) : InputStream() {
    private val buffer = ByteArray(16384)
    private var position = 0
    private var limit = 0
    override fun read(): Int {
        val byte = ByteArray(1)
        return if (read(byte, 0, 1) < 0) -1 else byte[0].toInt() and 255
    }
    override fun read(dest: ByteArray, offset: Int, length: Int): Int {
        require(offset >= 0 && length >= 0 && offset <= dest.size - length)
        if (length == 0) return 0
        while (position == limit) {
            limit = input.read(buffer)
            position = 0
            if (limit < 0) return -1
        }
        val count = minOf(length, limit - position)
        buffer.copyInto(dest, offset, position, position + count)
        position += count
        return count
    }
}
internal class AccessoryOutput(private val output: OutputStream) : OutputStream() {
    override fun write(value: Int) { write(byteArrayOf(value.toByte())) }
    override fun write(source: ByteArray, offset: Int, length: Int) {
        require(offset >= 0 && length >= 0 && offset <= source.size - length)
        var position = offset
        val end = offset + length
        while (position < end) {
            val count = minOf(16384, end - position)
            output.write(source, position, count)
            position += count
        }
    }
    override fun flush() { output.flush() }
}
