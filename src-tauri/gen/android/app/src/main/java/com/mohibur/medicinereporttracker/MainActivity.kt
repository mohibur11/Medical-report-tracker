package com.mohibur.medicinereporttracker

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.OpenableColumns
import androidx.activity.enableEdgeToEdge
import java.io.File

/**
 * Anything shared to this app is copied somewhere the Rust side can find it.
 *
 * The web view cannot read a `content://` URI and neither can `std::fs`, so the
 * bytes are pulled out here, written into the app's own storage under a real
 * name, and left for the app to pick up when it next looks. That keeps the whole
 * of ingest — hashing, de-duplication, orientation — exactly as it is on the
 * desktop rather than growing a second path for shared files.
 */
class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    receive(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    receive(intent)
  }

  private fun receive(intent: Intent?) {
    if (intent == null) return

    val uris: List<Uri> = when (intent.action) {
      Intent.ACTION_SEND -> listOfNotNull(uriFrom(intent, Intent.EXTRA_STREAM))
      Intent.ACTION_SEND_MULTIPLE -> urisFrom(intent)
      else -> emptyList()
    }
    if (uris.isEmpty()) return

    val inbox = File(filesDir, "shared-inbox")
    inbox.mkdirs()
    for (uri in uris) {
      runCatching {
        val name = displayName(uri) ?: "shared-${System.currentTimeMillis()}"
        // Prefixed with the clock so two shares of the same filename cannot
        // overwrite one another before either is imported.
        val target = File(inbox, "${System.currentTimeMillis()}-$name")
        contentResolver.openInputStream(uri)?.use { input ->
          target.outputStream().use { output -> input.copyTo(output) }
        }
      }
    }
  }

  @Suppress("DEPRECATION")
  private fun uriFrom(intent: Intent, key: String): Uri? =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      intent.getParcelableExtra(key, Uri::class.java)
    } else {
      intent.getParcelableExtra(key)
    }

  @Suppress("DEPRECATION")
  private fun urisFrom(intent: Intent): List<Uri> =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java) ?: emptyList()
    } else {
      intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM) ?: emptyList()
    }

  /** The name the sending app gave it, so the imported file is recognisable. */
  private fun displayName(uri: Uri): String? =
    contentResolver.query(uri, null, null, null, null)?.use { cursor ->
      val column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
      if (column >= 0 && cursor.moveToFirst()) cursor.getString(column) else null
    }
}
