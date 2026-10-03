package io.github.tymonoman.extraspace

internal class TouchTracker {
    private val active = linkedMapOf<Int, TouchEvent>()
    fun record(event: TouchEvent): TouchEvent? {
        when (event.action) {
            Protocol.TouchAction.DOWN -> active[event.slot] = event
            Protocol.TouchAction.MOTION -> {
                if (!active.containsKey(event.slot)) return null
                active[event.slot] = event
            }
            Protocol.TouchAction.UP -> if (active.remove(event.slot) == null) return null
        }
        return event
    }
    fun cancel(): List<TouchEvent> {
        val released = active.values.map { it.copy(action = Protocol.TouchAction.UP) }
        active.clear()
        return released
    }
}
