package dev.scenesync.player

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.util.AttributeSet
import android.view.View
import dev.scenesync.engine.Mapping

/**
 * The fit-to-window calibration pattern: a white border around the whole
 * projector image, a 10% grid, and numbered corners (1 top-left, 2 top-right,
 * 3 bottom-right, 4 bottom-left). In the dashboard you click those four
 * corners in a photo of the wall, then the window's corners. If a mapping is
 * already set, its shape is drawn in orange so corners can be nudged live.
 */
class CalibrationView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs) {
    var mapping: Mapping? = null
        set(value) {
            if (field != value) {
                field = value
                invalidate()
            }
        }
    var label: String = ""
        set(value) {
            if (field != value) {
                field = value
                invalidate()
            }
        }

    private val grid = Paint().apply { color = Color.rgb(70, 70, 70); strokeWidth = 2f }
    private val border = Paint().apply { color = Color.WHITE; style = Paint.Style.STROKE; strokeWidth = 6f }
    private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.WHITE; textSize = 28f }
    private val number = Paint(Paint.ANTI_ALIAS_FLAG).apply { textSize = 64f; isFakeBoldText = true }
    private val bracket = Paint().apply { style = Paint.Style.STROKE; strokeWidth = 10f }
    private val quadLine = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(249, 115, 22); style = Paint.Style.STROKE; strokeWidth = 5f
    }
    private val quadFill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.argb(70, 249, 115, 22) }
    private val cornerColors = intArrayOf(
        Color.rgb(239, 68, 68), Color.rgb(34, 197, 94), Color.rgb(59, 130, 246), Color.rgb(250, 204, 21),
    )

    override fun onDraw(canvas: Canvas) {
        val w = width.toFloat()
        val h = height.toFloat()
        canvas.drawColor(Color.BLACK)
        for (i in 1..9) {
            canvas.drawLine(w * i / 10, 0f, w * i / 10, h, grid)
            canvas.drawLine(0f, h * i / 10, w, h * i / 10, grid)
        }
        canvas.drawRect(3f, 3f, w - 3f, h - 3f, border)

        // Numbered corner brackets, drawn right at the image corners.
        val arm = minOf(w, h) * 0.08f
        val corners = arrayOf(floatArrayOf(0f, 0f), floatArrayOf(w, 0f), floatArrayOf(w, h), floatArrayOf(0f, h))
        corners.forEachIndexed { i, (x, y) ->
            val sx = if (x == 0f) 1f else -1f
            val sy = if (y == 0f) 1f else -1f
            val cx = x + sx * 5f
            val cy = y + sy * 5f
            bracket.color = cornerColors[i]
            canvas.drawLine(cx, cy, cx + sx * arm, cy, bracket)
            canvas.drawLine(cx, cy, cx, cy + sy * arm, bracket)
            number.color = cornerColors[i]
            val tx = cx + sx * (arm * 0.35f) - (if (sx < 0) number.measureText("${i + 1}") else 0f)
            val ty = cy + sy * (arm * 0.35f) + (if (sy > 0) number.textSize else 0f)
            canvas.drawText("${i + 1}", tx, ty, number)
        }

        // Centre cross and caption.
        canvas.drawLine(w / 2 - 30, h / 2, w / 2 + 30, h / 2, border)
        canvas.drawLine(w / 2, h / 2 - 30, w / 2, h / 2 + 30, border)
        val caption = "SceneSync fit to window" + if (label.isNotEmpty()) ": $label" else ""
        canvas.drawText(caption, w / 2 - text.measureText(caption) / 2, h / 2 + 80, text)

        mapping?.corners?.takeIf { it.size == 4 }?.let { c ->
            val path = Path()
            c.forEachIndexed { i, p ->
                val x = (p[0] * w).toFloat()
                val y = (p[1] * h).toFloat()
                if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
            }
            path.close()
            canvas.drawPath(path, quadFill)
            canvas.drawPath(path, quadLine)
        }
    }
}
