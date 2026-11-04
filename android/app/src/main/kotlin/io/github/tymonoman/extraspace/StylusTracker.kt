package io.github.tymonoman.extraspace

import kotlin.math.abs

/** Tracks pen contact and proximity independently of finger touch slots. */
internal class StylusTracker {
    var hovering: Boolean = false
        private set
    var down: Boolean = false
        private set
    private var lastEvent: StylusEvent? = null

    val isActive: Boolean
        get() = hovering || down

    fun record(event: StylusEvent): StylusEvent? {
        val prev = lastEvent
        return when (event.action) {
            Protocol.StylusAction.HOVER -> {
                hovering = true
                down = false
                val normalized = event.copy(pressure = 0f)
                if (prev != null &&
                    prev.action == Protocol.StylusAction.HOVER &&
                    prev.tool == normalized.tool &&
                    prev.buttons == normalized.buttons &&
                    abs(prev.x - normalized.x) < 1e-6 &&
                    abs(prev.y - normalized.y) < 1e-6 &&
                    abs(prev.tiltX - normalized.tiltX) < 1e-3f &&
                    abs(prev.tiltY - normalized.tiltY) < 1e-3f
                ) {
                    null
                } else {
                    lastEvent = normalized
                    normalized
                }
            }
            Protocol.StylusAction.DOWN -> {
                hovering = true
                down = true
                lastEvent = event
                event
            }
            Protocol.StylusAction.MOTION -> {
                if (!down) return null
                if (prev != null &&
                    prev.tool == event.tool &&
                    prev.buttons == event.buttons &&
                    abs(prev.x - event.x) < 1e-6 &&
                    abs(prev.y - event.y) < 1e-6 &&
                    abs(prev.pressure - event.pressure) < 1e-3f &&
                    abs(prev.tiltX - event.tiltX) < 1e-3f &&
                    abs(prev.tiltY - event.tiltY) < 1e-3f
                ) {
                    null
                } else {
                    lastEvent = event
                    event
                }
            }
            Protocol.StylusAction.UP -> {
                if (!down) return null
                down = false
                val released = event.copy(pressure = 0f)
                lastEvent = released
                released
            }
            Protocol.StylusAction.HOVER_EXIT -> {
                if (!hovering && !down) return null
                val exited = event.copy(pressure = 0f, buttons = 0)
                hovering = false
                down = false
                lastEvent = null
                exited
            }
            else -> null
        }
    }

    fun cancel(): List<StylusEvent> {
        val prev = lastEvent ?: run {
            hovering = false
            down = false
            return emptyList()
        }
        val out = ArrayList<StylusEvent>(2)
        if (down) {
            out.add(prev.copy(action = Protocol.StylusAction.UP, pressure = 0f, buttons = 0))
        }
        if (hovering || down) {
            out.add(prev.copy(action = Protocol.StylusAction.HOVER_EXIT, pressure = 0f, buttons = 0))
        }
        hovering = false
        down = false
        lastEvent = null
        return out
    }
}
