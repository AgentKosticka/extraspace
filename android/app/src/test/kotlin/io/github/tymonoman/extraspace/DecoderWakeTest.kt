package io.github.tymonoman.extraspace

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

class DecoderWakeTest {
    @Test fun idleWaitParksUntilInputAndShutdownWakesIt() {
        val wake = DecoderWake()
        val parked = CountDownLatch(1)
        val resumed = CountDownLatch(1)
        val shutdown = AtomicBoolean(false)
        val worker = thread(isDaemon = true) {
            if (wake.awaitWork { parked.countDown(); false }) resumed.countDown()
            shutdown.set(!wake.awaitWork { false })
        }
        try {
            assertTrue(parked.await(1, TimeUnit.SECONDS))
            assertFalse(resumed.await(100, TimeUnit.MILLISECONDS))
            wake.signal()
            assertTrue(resumed.await(1, TimeUnit.SECONDS))
        } finally {
            wake.close()
            worker.join(1000)
        }
        assertFalse(worker.isAlive)
        assertTrue(shutdown.get())
    }

    @Test fun earlyInputIsRetainedAndRepeatedSignalsCoalesce() {
        val wake = DecoderWake()
        repeat(1000) { wake.signal() }
        assertTrue(wake.awaitWork { false })
        val parked = CountDownLatch(1)
        val resumed = AtomicBoolean(false)
        val worker = thread(isDaemon = true) {
            resumed.set(wake.awaitWork { parked.countDown(); false })
        }
        try {
            assertTrue(parked.await(1, TimeUnit.SECONDS))
            assertFalse(resumed.get())
        } finally {
            wake.close()
            worker.join(1000)
        }
        assertFalse(resumed.get())
        assertFalse(worker.isAlive)
    }

    @Test fun pendingPicturesDrainWithoutWaitingForAnotherInput() {
        val wake = DecoderWake()
        assertTrue(wake.awaitWork { true })
        wake.close()
        wake.signal()
        assertFalse(wake.awaitWork { true })
    }
}
