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
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL

@InvokeArg
class KeepAliveArgs {
  var sessions: Int = 0
  var text: String = ""
}

@InvokeArg
class WatchArgs {
  lateinit var channel: Channel
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
  fun watchDisconnect(invoke: Invoke) {
    val args = invoke.parseArgs(WatchArgs::class.java)
    val channel = args.channel
    SessionService.onDisconnect = { channel.send(JSObject()) }
    invoke.resolve()
  }

  // Looks up the latest release on GitHub with Android's own HTTPS stack, off the main thread.
  @Command
  fun latestRelease(invoke: Invoke) {
    Thread {
      try {
        val conn = URL(LATEST_RELEASE).openConnection() as HttpURLConnection
        try {
          conn.connectTimeout = 10_000
          conn.readTimeout = 15_000
          conn.setRequestProperty("Accept", "application/vnd.github+json")
          conn.setRequestProperty("User-Agent", "Citadel-Android")
          val code = conn.responseCode
          if (code != 200) {
            invoke.reject("GitHub answered $code")
          } else {
            val body = conn.inputStream.use { readCapped(it, MAX_RELEASE_BYTES) }
            invoke.resolve(JSObject().put("body", body))
          }
        } finally {
          conn.disconnect()
        }
      } catch (e: Exception) {
        invoke.reject(e.message ?: "could not reach GitHub")
      }
    }.start()
  }

  private fun readCapped(input: InputStream, cap: Int): String {
    val out = ByteArrayOutputStream()
    val buf = ByteArray(8192)
    while (true) {
      val n = input.read(buf)
      if (n < 0) break
      if (out.size() + n > cap) throw IOException("the release information is too large")
      out.write(buf, 0, n)
    }
    return out.toString("UTF-8")
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
    private const val LATEST_RELEASE = "https://api.github.com/repos/sepehrmohseni/citadel/releases/latest"
    private const val MAX_RELEASE_BYTES = 2 * 1024 * 1024
  }
}
