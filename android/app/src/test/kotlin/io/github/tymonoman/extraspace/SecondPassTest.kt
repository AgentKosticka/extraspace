package io.github.tymonoman.extraspace

import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.Collections

class SecondPassTest {
    private fun shape() = CursorUpdate(true, true, 10, 20, true, 2, 3, byteArrayOf(1, 2, 3, 4), 1, 1)
    private fun move() = CursorUpdate(true, true, 50, 60, false, 0, 0, null, 0, 0)
    @Test fun cursorShapeAndHotspotSurviveMoveAndHideBursts() {
        val combined = mergeCursor(shape(), move())
        assertArrayEquals(shape().bitmap, combined.bitmap)
        assertEquals(2, combined.hotX); assertEquals(3, combined.hotY)
        assertEquals(50, combined.x); assertEquals(60, combined.y)
        val hidden = mergeCursor(shape(), CursorUpdate.hide())
        assertFalse(hidden.visible); assertNotNull(hidden.bitmap)
        val visible = mergeCursor(hidden, move())
        assertTrue(visible.visible); assertArrayEquals(shape().bitmap, visible.bitmap)
    }
    @Test fun newShapeReplacesOldShapeAndPendingPosition() {
        val next = shape().copy(bitmap = byteArrayOf(5, 6, 7, 8), hotX = 9)
        val combined = mergeCursor(move(), next)
        assertArrayEquals(next.bitmap, combined.bitmap)
        assertEquals(9, combined.hotX); assertEquals(10, combined.x)
    }
    @Test fun videoAspectFitWorksForMirrorAndRotatedSources() {
        assertEquals(2880 to 1620, fitVideo(2880, 1800, 1920, 1080))
        assertEquals(1013 to 1800, fitVideo(2880, 1800, 1080, 1920))
        assertEquals(2880 to 1800, fitVideo(2880, 1800, 1920, 1200))
    }
    @Test fun noPictureAndPendingWorkTimeOutButIdleDesktopDoesNot() {
        val watch = PresentationWatch(0)
        assertNull(watch.failure(7_999_999_999, 0, 1))
        assertNotNull(watch.failure(8_000_000_000, 0, 1))
        watch.lastOutputNs = 9_000_000_000
        assertNull(watch.failure(60_000_000_000, 1, 0))
        assertNotNull(watch.failure(12_000_000_000, 1, 1))
    }
    @Test fun stalledWriterDoesNotBlockSubmissionAndWatchdogEndsConnection() {
        val entered = CountDownLatch(1); val unblock = CountDownLatch(1); val failed = CountDownLatch(1)
        val output = OutputDispatcher("test-output", { failed.countDown(); unblock.countDown() }, timeoutMs = 150)
        try {
            output.submit { entered.countDown(); unblock.await() }
            assertTrue(entered.await(1, TimeUnit.SECONDS))
            val start = System.nanoTime()
            output.submit { }
            assertTrue(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start) < 100)
            assertTrue(failed.await(2, TimeUnit.SECONDS))
        } finally { unblock.countDown(); output.close() }
    }
    @Test fun coalescingKeepsGestureBoundariesAndFinalPositionOrdered() {
        val entered = CountDownLatch(1); val unblock = CountDownLatch(1); val done = CountDownLatch(1)
        val results = Collections.synchronizedList(mutableListOf<String>())
        val output = OutputDispatcher("test-order", { fail(it) })
        try {
            output.submit { entered.countDown(); unblock.await() }
            assertTrue(entered.await(1, TimeUnit.SECONDS))
            output.submit { results.add("down") }
            repeat(20) { i -> output.submit("motion:1") { results.add("move:$i") } }
            output.submit { results.add("up") }
            output.submit { results.add("down-again"); done.countDown() }
            unblock.countDown()
            assertTrue(done.await(2, TimeUnit.SECONDS))
            assertEquals(listOf("down", "move:19", "up", "down-again"), results.toList())
        } finally { unblock.countDown(); output.close() }
    }
    @Test fun boundedQueueFailsRatherThanDroppingGestureBoundaries() {
        val entered = CountDownLatch(1); val unblock = CountDownLatch(1); val failed = CountDownLatch(1)
        val output = OutputDispatcher("test-bound", { failed.countDown(); unblock.countDown() }, capacity = 2)
        try {
            output.submit { entered.countDown(); unblock.await() }
            assertTrue(entered.await(1, TimeUnit.SECONDS))
            repeat(3) { output.submit { } }
            assertTrue(failed.await(1, TimeUnit.SECONDS))
        } finally { unblock.countDown(); output.close() }
    }
}
