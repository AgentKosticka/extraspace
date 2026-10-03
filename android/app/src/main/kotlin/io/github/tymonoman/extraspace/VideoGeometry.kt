package io.github.tymonoman.extraspace

import kotlin.math.roundToInt

/** Shared aspect-fit size for the video layer, touch, and cursor. */
fun fitVideo(viewWidth: Int, viewHeight: Int, streamWidth: Int, streamHeight: Int): Pair<Int, Int> {
    if (viewWidth <= 0 || viewHeight <= 0 || streamWidth <= 0 || streamHeight <= 0)
        return viewWidth to viewHeight
    val scale = minOf(viewWidth.toDouble() / streamWidth, viewHeight.toDouble() / streamHeight)
    return (streamWidth * scale).roundToInt().coerceIn(1, viewWidth) to
        (streamHeight * scale).roundToInt().coerceIn(1, viewHeight)
}
