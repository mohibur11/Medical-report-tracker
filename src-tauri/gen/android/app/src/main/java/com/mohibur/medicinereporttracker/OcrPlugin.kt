package com.mohibur.medicinereporttracker

import android.app.Activity
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.result.ActivityResult
import androidx.browser.customtabs.CustomTabsIntent
import app.tauri.annotation.ActivityCallback
import java.io.File
import java.net.InetAddress
import java.net.ServerSocket
import java.net.SocketTimeoutException
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.android.gms.tasks.Tasks
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.latin.TextRecognizerOptions

@InvokeArg
class RecognizeArgs {
  lateinit var path: String
}

@InvokeArg
class OpenArgs {
  lateinit var url: String
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
/** HTTP wants its line endings exactly, and no browser is forgiving about it. */
private const val CRLF = "\r\n"

private const val DONE_PAGE =
  "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>" +
    "<body style='font:16px system-ui;padding:3rem;text-align:center'>" +
    "<h2>Signed in</h2><p>Close this tab and go back to the app.</p>"

/** Served to anything that is not the redirect, so the browser is never left hanging. */
private const val WAIT_PAGE =
  "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>" +
    "<body style='font:16px system-ui;padding:3rem;text-align:center'>" +
    "<p>Waiting for Google…</p>"

@TauriPlugin
class OcrPlugin(private val activity: Activity) : Plugin(activity) {
  private val recognizer = TextRecognition.getClient(TextRecognizerOptions.DEFAULT_OPTIONS)

  /** Open only between starting a sign-in and its answer arriving. */
  private var loopback: ServerSocket? = null

  /**
   * Choose reports from the phone's storage.
   *
   * The Tauri file dialog hands back a `content://` URI, which nothing on the
   * Rust side can open — which is why the Add button appeared to do nothing. The
   * bytes are copied into the same inbox a shared file lands in, so both routes
   * end up in one place and go through one ingest.
   */
  /**
   * Listen for Google's answer to a sign-in, on this phone.
   *
   * The socket is opened here rather than in Rust. An identical listener written
   * in Rust bound its port and then never accepted anything — the browser sat on
   * the connection backlog until it gave up — and the sign-in hung four times
   * over. Whatever the reason, the platform runs its own threads reliably, so
   * this half lives on the platform's side of the line.
   *
   * Port 0 asks the system for a free one. Google accepts any port on loopback
   * for a desktop client, which is why that client is the one used here: an
   * Android client cannot be used against the browser authorization endpoint at
   * all, and asking it to be produces `Error 400: invalid_request`.
   */
  @Command
  fun startLoopback(invoke: Invoke) {
    try {
      runCatching { loopback?.close() }
      // Backlog of one: exactly one browser is coming back.
      val socket = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))
      loopback = socket

      val response = JSObject()
      response.put("port", socket.localPort)
      invoke.resolve(response)
    } catch (e: Exception) {
      invoke.reject(e.message ?: "cannot listen for Google's reply")
    }
  }

  /**
   * Wait for the redirect and hand back its query string.
   *
   * Accepts in a loop rather than once: a browser opens connections this app did
   * not ask for — a favicon, a speculative pre-connect — and the first attempt
   * treated one of those as the answer and stopped listening. Only a request
   * carrying `code` or `error` ends it.
   *
   * Runs on its own thread, and resolves on the UI thread, because the answer
   * takes as long as somebody takes to choose an account and press Allow.
   */
  @Command
  fun awaitRedirect(invoke: Invoke) {
    val socket = loopback
    if (socket == null) {
      invoke.reject("the sign-in was not started")
      return
    }

    Thread {
      try {
        // The whole sign-in, not one connection: the person has to read a consent
        // screen, and possibly type a password, before anything arrives.
        socket.soTimeout = 180_000

        var query: String? = null
        while (query == null) {
          socket.accept().use { client ->
            val line = client.getInputStream().bufferedReader().readLine() ?: ""
            // "GET /?code=...&state=... HTTP/1.1"
            val target = line.split(' ').getOrNull(1) ?: ""
            val found = target.substringAfter('?', "")
            val done = found.contains("code=") || found.contains("error=")

            val body = if (done) DONE_PAGE else WAIT_PAGE
            val bytes = body.toByteArray(Charsets.UTF_8)
            val head = listOf(
              "HTTP/1.1 200 OK",
              "Content-Type: text/html; charset=utf-8",
              "Content-Length: ${bytes.size}",
              "Connection: close",
              "",
              "",
            ).joinToString(CRLF)

            client.getOutputStream().apply {
              write(head.toByteArray(Charsets.UTF_8))
              write(bytes)
              flush()
            }

            if (done) query = found
          }
        }

        val response = JSObject()
        response.put("query", query)
        activity.runOnUiThread { invoke.resolve(response) }
      } catch (e: SocketTimeoutException) {
        activity.runOnUiThread {
          invoke.reject("Google did not answer within three minutes. Try again.")
        }
      } catch (e: Exception) {
        activity.runOnUiThread { invoke.reject(e.message ?: "the sign-in did not come back") }
      } finally {
        runCatching { socket.close() }
        loopback = null
      }
    }.start()
  }

  /**
   * Open the Google sign-in without leaving the app.
   *
   * Handing the URL to the system browser sends this app to the background, and
   * Android freezes a cached process — which stops the loopback listener waiting
   * for Google's answer from accepting anything, so the redirect arrives at a
   * port that never replies. A Custom Tab runs in this app's own task, so the
   * process stays awake and the listener keeps running.
   */
  @Command
  fun openAuth(invoke: Invoke) {
    val args = invoke.parseArgs(OpenArgs::class.java)
    try {
      CustomTabsIntent.Builder()
        .setShowTitle(true)
        .build()
        .launchUrl(activity, Uri.parse(args.url))
      invoke.resolve()
    } catch (e: Exception) {
      // No browser that supports Custom Tabs. Falling back to whatever will open
      // it is better than refusing, even though the freeze may then apply.
      try {
        activity.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(args.url)))
        invoke.resolve()
      } catch (inner: Exception) {
        invoke.reject(inner.message ?: "cannot open a browser")
      }
    }
  }

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

  /**
   * Read every page of a PDF.
   *
   * The desktop extracts page images with pdfcpu, which cannot run on Android at
   * all — so without this a PDF from a lab was filed with no text, no date and no
   * suggested title. PdfRenderer is built into the platform and does the same job:
   * each page is drawn to a bitmap and handed to the same recogniser a photograph
   * goes through.
   */
  @Command
  fun recognizePdf(invoke: Invoke) {
    val args = invoke.parseArgs(RecognizeArgs::class.java)
    val started = System.currentTimeMillis()
    val file = File(args.path)
    if (!file.exists()) {
      invoke.reject("cannot find the PDF at ${args.path}")
      return
    }

    var descriptor: ParcelFileDescriptor? = null
    var renderer: PdfRenderer? = null
    try {
      descriptor = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
      renderer = PdfRenderer(descriptor)

      val pages = JSArray()
      for (index in 0 until renderer.pageCount) {
        renderer.openPage(index).use { page ->
          // Rendered at roughly 200 DPI. Below that a printed lab report starts
          // losing the small print that carries the reference ranges.
          val scale = 200f / 72f
          val bitmap = Bitmap.createBitmap(
            (page.width * scale).toInt().coerceAtLeast(1),
            (page.height * scale).toInt().coerceAtLeast(1),
            Bitmap.Config.ARGB_8888,
          )
          // PdfRenderer draws only ink; without this the page is transparent and
          // recognition sees nothing.
          bitmap.eraseColor(Color.WHITE)
          page.render(bitmap, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)

          val result = Tasks.await(recognizer.process(InputImage.fromBitmap(bitmap, 0)))
          bitmap.recycle()

          val entry = JSObject()
          entry.put("text", result.text)
          entry.put("words", JSArray())
          entry.put("engine", "mlkit-latin/pdfrenderer")
          entry.put("millis", System.currentTimeMillis() - started)
          entry.put("pageNo", index + 1)
          pages.put(entry)
        }
      }

      val response = JSObject()
      response.put("pages", pages)
      invoke.resolve(response)
    } catch (e: Exception) {
      invoke.reject(e.message ?: "cannot read this PDF")
    } finally {
      renderer?.close()
      descriptor?.close()
    }
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
