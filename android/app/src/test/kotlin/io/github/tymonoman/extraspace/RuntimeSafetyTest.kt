package io.github.tymonoman.extraspace

import org.junit.Assert.*
import org.junit.Test
import java.nio.ByteBuffer
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

class RuntimeSafetyTest {
    @Test fun cancelReleasesEveryFingerOnceIncludingMovedPositions() {
        val tracker = TouchTracker()
        for (slot in listOf(2, 7, 13)) tracker.record(TouchEvent(Protocol.TouchAction.DOWN, slot, 1.0, 2.0))
        tracker.record(TouchEvent(Protocol.TouchAction.MOTION, 7, 40.0, 50.0))
        val released = tracker.cancel()
        assertEquals(setOf(2, 7, 13), released.map { it.slot }.toSet())
        assertTrue(released.all { it.action == Protocol.TouchAction.UP })
        assertEquals(40.0, released.single { it.slot == 7 }.x, 0.0)
        assertTrue(tracker.cancel().isEmpty())
        assertNull(tracker.record(TouchEvent(Protocol.TouchAction.MOTION, 7, 1.0, 1.0)))
    }

    @Test fun liftedFingerIsNotReleasedTwiceOnCancel() {
        val tracker = TouchTracker()
        tracker.record(TouchEvent(Protocol.TouchAction.DOWN, 2, 1.0, 2.0))
        tracker.record(TouchEvent(Protocol.TouchAction.DOWN, 7, 1.0, 2.0))
        tracker.record(TouchEvent(Protocol.TouchAction.UP, 2, 1.0, 2.0))
        assertEquals(listOf(7), tracker.cancel().map { it.slot })
    }

    @Test fun stationaryFingerMotionsAreSkippedWhileMovingFingerEmits() {
        val tracker = TouchTracker()
        assertNotNull(tracker.record(TouchEvent(Protocol.TouchAction.DOWN, 0, 10.0, 20.0)))
        assertNotNull(tracker.record(TouchEvent(Protocol.TouchAction.DOWN, 1, 50.0, 60.0)))
        assertNull(tracker.record(TouchEvent(Protocol.TouchAction.MOTION, 0, 10.0, 20.0)))
        assertNotNull(tracker.record(TouchEvent(Protocol.TouchAction.MOTION, 1, 51.0, 60.0)))
    }

    @Test fun stylusTrackerTracksHoverPressureTiltAndCancelsCleanly() {
        val stylus = StylusTracker()
        assertFalse(stylus.isActive)
        val hover = StylusEvent(Protocol.StylusAction.HOVER, Protocol.StylusTool.PEN, 0, 100.0, 200.0, 0.5f, 0.1f, -0.2f)
        val recordedHover = stylus.record(hover)!!
        assertEquals(0f, recordedHover.pressure, 0f)
        assertTrue(stylus.isActive)
        assertNull(stylus.record(hover))
        val down = StylusEvent(Protocol.StylusAction.DOWN, Protocol.StylusTool.PEN, Protocol.StylusButtons.PRIMARY, 100.0, 200.0, 0.4f, 0.1f, -0.2f)
        assertEquals(down, stylus.record(down))
        val pressureChange = down.copy(action = Protocol.StylusAction.MOTION, pressure = 0.85f)
        assertEquals(pressureChange, stylus.record(pressureChange))
        assertNull(stylus.record(pressureChange))
        val cancelled = stylus.cancel()
        assertEquals(listOf(Protocol.StylusAction.UP, Protocol.StylusAction.HOVER_EXIT), cancelled.map { it.action })
        assertFalse(stylus.isActive)
    }

    @Test fun concurrentFailuresHaveExactlyOneOwnerAndCannotRestart() {
        val state = ConnectionState()
        assertTrue(state.start())
        val go = CountDownLatch(1)
        val owners = AtomicInteger(0)
        val workers = List(16) { thread {
            go.await()
            if (state.finish()) owners.incrementAndGet()
        } }
        go.countDown()
        workers.forEach { it.join() }
        assertEquals(1, owners.get())
        assertFalse(state.get())
        assertFalse(state.start())
        assertFalse(state.finish())
    }

    @Test fun closedManagerCannotStartLater() {
        val state = ConnectionState()
        state.finish()
        assertFalse(state.start())
    }

    @Test fun decoderRejectsSaturationNullUndersizedAndInvalidInput() {
        val bytes = byteArrayOf(1, 2, 3, 4)
        assertThrows(ProtocolException::class.java) { DecoderInput.fill(bytes, 4, { -1 }, { error("must not request a buffer") }) }
        assertThrows(ProtocolException::class.java) { DecoderInput.fill(bytes, 4, { 0 }, { null }) }
        val short = ByteBuffer.allocate(3)
        assertThrows(ProtocolException::class.java) { DecoderInput.fill(bytes, 4, { 0 }, { short }) }
        assertEquals(0, short.position())
        assertThrows(ProtocolException::class.java) { DecoderInput.fill(bytes, 5, { error("invalid length must fail first") }, { null }) }
        assertThrows(ProtocolException::class.java) { DecoderInput.fill(bytes, 0, { error("invalid length must fail first") }, { null }) }
        val input = ByteBuffer.allocate(4)
        input.position(3)
        assertEquals(2, DecoderInput.fill(bytes, 4, { 2 }, { input }))
        assertArrayEquals(bytes, input.array())
        assertEquals(4, input.position())
    }
}
