package io.github.tymonoman.extraspace

import java.io.Closeable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** Bounded, nonblocking submission; a separate watchdog closes stalled sockets. */
class OutputDispatcher(private val name: String, private val onFailure: (String) -> Unit,
    private val capacity: Int = 64, private val timeoutMs: Long = 2000) : Closeable {
    private data class Job(val key: String?, val write: () -> Unit)
    private val lock = Object()
    private val jobs = java.util.ArrayDeque<Job>()
    private var closed = false
    @Volatile private var startedNs = 0L
    private val watchdog = Executors.newSingleThreadScheduledExecutor { job ->
        Thread(job, "$name-watchdog").apply { isDaemon = true }
    }
    private val worker = Thread({
        while (true) {
            val job = synchronized(lock) {
                while (!closed && jobs.isEmpty()) lock.wait()
                if (closed) return@Thread
                jobs.removeFirst()
            }
            startedNs = System.nanoTime()
            try { job.write() }
            catch (e: Exception) { fail(e.message ?: "Connection output failed") }
            finally { startedNs = 0 }
        }
    }, name).apply { isDaemon = true; start() }
    init {
        watchdog.scheduleWithFixedDelay({
            val since = startedNs
            if (since != 0L && System.nanoTime() - since > TimeUnit.MILLISECONDS.toNanos(timeoutMs))
                fail("Connection output stalled. Reconnect your computer.")
        }, 100, 100, TimeUnit.MILLISECONDS)
    }
    fun submit(key: String? = null, write: () -> Unit) {
        val full = synchronized(lock) {
            if (closed) return
            // Only adjacent replaceable jobs are merged; gesture boundaries stay ordered.
            if (key != null && jobs.peekLast()?.key == key) jobs.removeLast()
            if (jobs.size >= capacity) true
            else { jobs.addLast(Job(key, write)); lock.notifyAll(); false }
        }
        if (full) {
            try { watchdog.execute { fail("Connection output is overloaded. Reconnect your computer.") } }
            catch (_: java.util.concurrent.RejectedExecutionException) { /* Already closed. */ }
        }
    }
    private fun fail(reason: String) {
        val notify = synchronized(lock) {
            if (closed) false else { closed = true; jobs.clear(); lock.notifyAll(); true }
        }
        if (notify) { watchdog.shutdown(); onFailure(reason) }
    }
    override fun close() {
        synchronized(lock) { closed = true; jobs.clear(); lock.notifyAll() }
        watchdog.shutdownNow()
        // Socket shutdown belongs to the owner. Never join a blocked writer on the UI thread.
    }
}
