package com.mohibur.medicinereporttracker

import android.app.Activity
import android.graphics.BitmapFactory
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.latin.TextRecognizerOptions

@InvokeArg
class RecognizeArgs {
  lateinit var path: String
}

/**
 * Reading text off a photograph, using ML Kit.
 *
 * The desktop app uses Windows.Media.Ocr, which has no equivalent here. ML Kit's
 * on-device Latin recogniser is the closest match: it runs offline, needs no
 * account, and costs nothing — which matters, because these are medical records
 * and none of them should leave the phone.
 *
 * Word boxes come back with the text. The review screen ranks several dates
 * against the words around them, so where a date sits on the page is part of
 * deciding whether it is the report's date or the patient's birthday.
 */
@TauriPlugin
class OcrPlugin(private val activity: Activity) : Plugin(activity) {
  private val recognizer = TextRecognition.getClient(TextRecognizerOptions.DEFAULT_OPTIONS)

  @Command
  fun recognize(invoke: Invoke) {
    val args = invoke.parseArgs(RecognizeArgs::class.java)
    val started = System.currentTimeMillis()

    val bitmap = BitmapFactory.decodeFile(args.path)
    if (bitmap == null) {
      invoke.reject("cannot read the image at ${args.path}")
      return
    }

    recognizer
      .process(InputImage.fromBitmap(bitmap, 0))
      .addOnSuccessListener { result ->
        val words = JSArray()
        for (block in result.textBlocks) {
          for (line in block.lines) {
            for (element in line.elements) {
              val box = element.boundingBox ?: continue
              val word = JSObject()
              word.put("text", element.text)
              word.put("x", box.left.toDouble())
              word.put("y", box.top.toDouble())
              word.put("w", box.width().toDouble())
              word.put("h", box.height().toDouble())
              words.put(word)
            }
          }
        }

        val page = JSObject()
        // Reading order, flattened the same way the Windows recogniser flattens it,
        // so the date ranker sees the same shape of text on both platforms.
        page.put("text", result.text)
        page.put("words", words)
        page.put("engine", "mlkit-latin")
        page.put("millis", System.currentTimeMillis() - started)
        page.put("pageNo", 1)

        bitmap.recycle()
        invoke.resolve(page)
      }
      .addOnFailureListener { error ->
        bitmap.recycle()
        invoke.reject(error.message ?: "recognition failed")
      }
  }
}
