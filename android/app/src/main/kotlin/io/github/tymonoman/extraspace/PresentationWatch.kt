package io.github.tymonoman.extraspace

/** Only pending work can time out after the first picture; an idle desktop is healthy. */
class PresentationWatch(private val startedNs: Long) {
    @Volatile var lastOutputNs = startedNs
    fun failure(nowNs: Long, decoded: Long, pending: Int): String? = when {
        decoded == 0L && nowNs - startedNs >= 8_000_000_000L -> "No video picture decoded. Reconnect or choose another encoder on your computer."
        decoded > 0L && pending > 0 && nowNs - lastOutputNs >= 3_000_000_000L -> "Video decoder stalled. Restarting the stream."
        else -> null
    }
}
