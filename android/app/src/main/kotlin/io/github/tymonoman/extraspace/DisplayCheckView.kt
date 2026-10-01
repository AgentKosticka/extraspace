package io.github.tymonoman.extraspace

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.view.MotionEvent
import android.view.View

/** Offline panel and touch check. Swipe to inspect mapping; tap Close to return. */
class DisplayCheckView(context: Context) : View(context) {
    init { minimumHeight = (280 * resources.displayMetrics.density).toInt() }
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private var pointX = -1f
    private var pointY = -1f
    override fun onDraw(canvas: Canvas) {
        val colors = intArrayOf(Color.RED, Color.GREEN, Color.BLUE, Color.WHITE, Color.GRAY, Color.BLACK)
        colors.forEachIndexed { i, color ->
            paint.color = color
            canvas.drawRect(i * width / 6f, 0f, (i + 1) * width / 6f, height.toFloat(), paint)
        }
        paint.color = Color.CYAN
        paint.strokeWidth = resources.displayMetrics.density * 2
        for (i in 0..10) {
            canvas.drawLine(width * i / 10f, 0f, width * i / 10f, height.toFloat(), paint)
            canvas.drawLine(0f, height * i / 10f, width.toFloat(), height * i / 10f, paint)
        }
        if (pointX >= 0) {
            paint.color = Color.MAGENTA
            canvas.drawCircle(pointX, pointY, 24 * resources.displayMetrics.density, paint)
        }
    }
    override fun onTouchEvent(event: MotionEvent): Boolean {
        pointX = event.x; pointY = event.y
        contentDescription = "Touch at ${pointX.toInt()}, ${pointY.toInt()} of $width by $height"
        invalidate()
        if (event.actionMasked == MotionEvent.ACTION_UP) performClick()
        return true
    }
    override fun performClick(): Boolean { super.performClick(); return true }
}
