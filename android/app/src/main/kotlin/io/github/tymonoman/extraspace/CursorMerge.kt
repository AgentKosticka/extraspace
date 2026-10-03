package io.github.tymonoman.extraspace

/** Messages are deltas: position updates must not erase a pending shape. */
fun mergeCursor(previous: CursorUpdate?, next: CursorUpdate): CursorUpdate {
    if (previous == null) return next
    return next.copy(
        hasPosition = next.hasPosition || previous.hasPosition,
        x = if (next.hasPosition) next.x else previous.x,
        y = if (next.hasPosition) next.y else previous.y,
        hasHotspot = next.hasHotspot || previous.hasHotspot,
        hotX = if (next.hasHotspot) next.hotX else previous.hotX,
        hotY = if (next.hasHotspot) next.hotY else previous.hotY,
        bitmap = next.bitmap ?: previous.bitmap,
        bitmapWidth = if (next.bitmap != null) next.bitmapWidth else previous.bitmapWidth,
        bitmapHeight = if (next.bitmap != null) next.bitmapHeight else previous.bitmapHeight,
    )
}
