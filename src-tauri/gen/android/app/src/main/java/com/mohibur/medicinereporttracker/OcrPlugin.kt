package com.mohibur.medicinereporttracker

import android.app.Activity
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import java.io.File
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

  /**
   * Choose reports from the phone's storage.
   *
   * The Tauri file dialog hands back a `content://` URI, which nothing on the
   * Rust side can open — which is why the Add button appeared to do nothing. The
   * bytes are copied into the same inbox a shared file lands in, so both routes
   * end up in one place and go through one ingest.
   */
  @Command
  fun pick(invoke: Invoke) {
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
      type = "*/*"
      addCategory(Intent.CATEGORY_OPENABLE)
      putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("image/*", "application/pdf"))
      putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
    }
    startActivityForResult(invoke, intent, "picked")
  }

  @ActivityCallback
  fun picked(invoke: Invoke, result: ActivityResult) {
    val data = result.data
    val uris = mutableListOf<Uri>()
    data?.clipData?.let { clip ->
      for (i in 0 until clip.itemCount) uris.add(clip.getItemAt(i).uri)
    }
    if (uris.isEmpty()) data?.data?.let { uris.add(it) }

    val inbox = File(activity.filesDir, "shared-inbox")
    inbox.mkdirs()
    var taken = 0
    for (uri in uris) {
      runCatching {
        val name = displayName(uri) ?: "picked-${System.currentTimeMillis()}"
        val target = File(inbox, "${System.currentTimeMillis()}-$name")
        activity.contentResolver.openInputStream(uri)?.use { input ->
          target.outputStream().use { output -> input.copyTo(output) }
        }
        taken += 1
      }
    }

    val response = JSObject()
    response.put("taken", taken)
    invoke.resolve(response)
  }

  private fun displayName(uri: Uri): String? =
    activity.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
      val column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
      if (column >= 0 && cursor.moveToFirst()) cursor.getString(column) else null
    }

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
