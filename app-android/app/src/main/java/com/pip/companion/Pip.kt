package com.pip.companion

import android.graphics.Bitmap
import android.util.Base64
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import org.json.JSONObject
import java.nio.ByteBuffer

/**
 * The companion, drawn by the same Rust that draws it on the PC.
 *
 * There is no Kotlin drawing code here on purpose. The creature is a hundred-odd numbers and a few
 * hundred lines of curves that took weeks to get right, and a second implementation would start
 * identical and drift the first time either side changed. Rust renders it into an RGBA square and
 * Compose shows the square.
 *
 * It renders once per change rather than on a timer: a phone is not a desktop, and a pet that
 * repainted itself sixty times a second in a list view would be a battery decision, not a drawing
 * one. Animation is deliberately not here.
 */
@Composable
fun Pip(settings: JSONObject, state: JSONObject, size: Dp = 96.dp, modifier: Modifier = Modifier) {
    val pet = settings.optJSONObject("pet")
    val character = pet?.optString("character") ?: "sprout"
    val color = pet?.optString("color") ?: "mint"
    val experience = state.optInt("experience", 0)
    // Pixels, not dp: the bitmap is drawn once at a fixed size and scaled by Compose. 3x covers
    // the densest phone screens without re-rendering per device.
    val px = (size.value * 3).toInt()

    val bitmap = remember(character, color, experience, px) {
        try {
            val reply = Native.ask(
                "draw",
                JSONObject()
                    .put("character", character)
                    .put("color", color)
                    .put("experience", experience)
                    .put("size", px),
            )
            val w = reply.getInt("w")
            val h = reply.getInt("h")
            val rgba = Base64.decode(reply.getString("rgba"), Base64.DEFAULT)
            // tiny-skia hands back premultiplied RGBA, which is exactly what ARGB_8888 holds.
            Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888).apply {
                copyPixelsFromBuffer(ByteBuffer.wrap(rgba))
            }
        } catch (e: Exception) {
            null // a companion that will not draw is not worth a crash; the reminders still work
        }
    }

    if (bitmap != null) {
        Image(
            bitmap = bitmap.asImageBitmap(),
            contentDescription = "Your companion",
            modifier = modifier.size(size),
            contentScale = ContentScale.Fit,
        )
    }
}
