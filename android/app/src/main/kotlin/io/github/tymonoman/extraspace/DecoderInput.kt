package io.github.tymonoman.extraspace

import java.nio.ByteBuffer

/** Validate an entire access unit before telling the codec it contains bytes. */
internal object DecoderInput {
    fun fill(data: ByteArray, length: Int, dequeue: () -> Int, buffer: (Int) -> ByteBuffer?): Int {
        if (length <= 0 || length > data.size) throw ProtocolException("Invalid video access unit length $length")
        val index = dequeue()
        if (index < 0) throw ProtocolException("Decoder input stalled; restart the stream for a fresh keyframe")
        val input = buffer(index) ?: throw ProtocolException("Decoder returned a null input buffer")
        input.clear()
        if (input.remaining() < length) throw ProtocolException("Video access unit exceeds decoder input capacity")
        input.put(data, 0, length)
        return index
    }
}
