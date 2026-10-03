package io.github.tymonoman.extraspace

import java.util.concurrent.atomic.AtomicBoolean

/** A manager is single-use. Only one worker may own its terminal transition. */
internal class ConnectionState {
    private var started = false
    private val active = AtomicBoolean(false)
    @Synchronized fun start(): Boolean {
        if (started) return false
        started = true
        active.set(true)
        return true
    }
    fun get(): Boolean = active.get()
    @Synchronized fun finish(): Boolean {
        started = true
        return active.compareAndSet(true, false)
    }
}
