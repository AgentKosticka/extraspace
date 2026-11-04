package io.github.tymonoman.extraspace

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.util.AttributeSet
import android.view.View

/**
 * Draws the pen's recent path straight from local input, so ink appears under
 * the nib before the desktop's own rendering makes the round trip over USB.
 *
 * Only a short tail is drawn and it fades out; by then the streamed picture
 * shows the real stroke. Purely visual: it never consumes input and knows
 * nothing about what the desktop application does with the pen.
 */
class InkOverlay @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs) {
    private class Sample(val x: Float, val y: Float, val pressure: Float, val atNs: Long, val stroke: Int)

    private val samples = ArrayDeque<Sample>()
    private var stroke = 0
    private var down = false
    private val density = resources.displayMetrics.density
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
        // Neutral mid-grey stays visible on light and dark desktops alike.
        color = 0xFF808080.toInt()
    }

    init {
        isClickable = false
        isFocusable = false
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
    }

    /** Adds a pen-down sample in this view's coordinates; [start] begins a new stroke. */
    fun add(x: Float, y: Float, pressure: Float, start: Boolean, atNs: Long = System.nanoTime()) {
        if (start || !down) stroke++
        down = true
        samples.addLast(Sample(x, y, pressure.coerceIn(0f, 1f), atNs, stroke))
        if (samples.size > MAX_SAMPLES) samples.removeFirst()
        postInvalidateOnAnimation()
    }

    fun lift() {
        down = false
    }

    fun clear() {
        samples.clear()
        down = false
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val now = System.nanoTime()
        while (samples.isNotEmpty() && now - samples.first().atNs > TAIL_NS) samples.removeFirst()
        var previous: Sample? = null
        for (sample in samples) {
            val from = previous
            previous = sample
            if (from == null || from.stroke != sample.stroke) continue
            val age = (now - sample.atNs).toFloat() / TAIL_NS
            paint.alpha = ((1f - age) * 255).toInt().coerceIn(0, 255)
            paint.strokeWidth = (1.5f + 2.5f * sample.pressure) * density
            canvas.drawLine(from.x, from.y, sample.x, sample.y, paint)
        }
        if (samples.isNotEmpty()) postInvalidateOnAnimation()
    }

    companion object {
        /** Longer than the measured capture-to-display path, short enough to never linger. */
        const val TAIL_NS = 150_000_000L
        private const val MAX_SAMPLES = 512
    }
}
