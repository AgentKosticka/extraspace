package io.github.tymonoman.extraspace

/** A coalesced input signal: no timer, lost early wakeups, or accumulating permits. */
internal class DecoderWake {
    private val lock = Object()
    private var signaled = false
    private var closed = false

    fun signal() = synchronized(lock) {
        if (!closed) {
            signaled = true
            lock.notifyAll()
        }
    }

    fun awaitWork(hasPending: () -> Boolean): Boolean = synchronized(lock) {
        while (!closed && !signaled && !hasPending()) lock.wait()
        signaled = false
        !closed
    }

    fun close() = synchronized(lock) {
        closed = true
        lock.notifyAll()
    }
}
