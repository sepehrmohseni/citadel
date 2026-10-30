package io.github.sepehrmohseni.citadel

import android.Manifest
import android.app.Activity
import android.content.ContentValues
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class KeepAliveArgs {
  var sessions: Int = 0
  var text: String = ""
}

@InvokeArg
class DownloadArgs {
  var name: String = ""
  var content: String = ""
  var mime: String = "text/plain"
}

@TauriPlugin
class CitadelPlugin(private val activity: Activity) : Plugin(activity) {
  private var askedNotifications = false

  @Command
  fun keepAlive(invoke: Invoke) {
    val args = invoke.parseArgs(KeepAliveArgs::class.java)
    val intent = Intent(activity, SessionService::class.java)
    try {
      if (args.sessions > 0) {
        askNotifications()
        intent.putExtra(SessionService.SESSIONS, args.sessions).putExtra(SessionService.TEXT, args.text)
        ContextCompat.startForegroundService(activity, intent)
      } else {
        activity.stopService(intent)
      }
      invoke.resolve()
    } catch (e: Exception) {
      invoke.reject(e.message ?: "could not keep sessions alive")
    }
  }

  @Command
  fun saveDownload(invoke: Invoke) {
    val args = invoke.parseArgs(DownloadArgs::class.java)
    try {
      val bytes = args.content.toByteArray(Charsets.UTF_8)
      val location = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        val values = ContentValues().apply {
          put(MediaStore.Downloads.DISPLAY_NAME, args.name)
          put(MediaStore.Downloads.MIME_TYPE, args.mime)
          put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/Citadel")
          put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val resolver = activity.contentResolver
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
          ?: throw IllegalStateException("Android refused to create the file")
        try {
          val out = resolver.openOutputStream(uri) ?: throw IllegalStateException("Android refused to open the file")
          out.use { it.write(bytes) }
          resolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
        } catch (e: Exception) {
          resolver.delete(uri, null, null)
          throw e
        }
        val shown = resolver.query(uri, arrayOf(MediaStore.Downloads.DISPLAY_NAME), null, null, null)?.use { c ->
          if (c.moveToFirst()) c.getString(0) else null
        } ?: args.name
        "Download/Citadel/$shown"
      } else {
        val dir = File(activity.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS), "Citadel")
        dir.mkdirs()
        val file = File(dir, args.name)
        file.writeBytes(bytes)
        file.absolutePath
      }
      invoke.resolve(JSObject().put("location", location))
    } catch (e: Exception) {
      invoke.reject(e.message ?: "could not save the file")
    }
  }

  private fun askNotifications() {
    if (askedNotifications || Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    askedNotifications = true
    if (ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
      ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), NOTIFICATIONS_REQUEST)
    }
  }

  companion object {
    private const val NOTIFICATIONS_REQUEST = 4101
  }
}
