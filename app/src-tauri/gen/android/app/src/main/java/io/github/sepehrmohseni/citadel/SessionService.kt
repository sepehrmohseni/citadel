package io.github.sepehrmohseni.citadel

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

class SessionService : Service() {
  private var wake: PowerManager.WakeLock? = null
  private val handler = Handler(Looper.getMainLooper())
  private val refresh = object : Runnable {
    override fun run() {
      wake?.acquire(WAKE_MS)
      handler.postDelayed(this, WAKE_MS / 2)
    }
  }
  private val forceStop = Runnable {
    ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
    stopSelf()
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_DISCONNECT) {
      disconnect()
      return START_NOT_STICKY
    }
    handler.removeCallbacks(forceStop)
    val sessions = intent?.getIntExtra(SESSIONS, 0) ?: 0
    val text = intent?.getStringExtra(TEXT).orEmpty()
    ensureChannel()
    val open = PendingIntent.getActivity(
      this,
      0,
      Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_REORDER_TO_FRONT),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val close = PendingIntent.getService(
      this,
      1,
      Intent(this, SessionService::class.java).setAction(ACTION_DISCONNECT),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val notification = NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_stat_citadel)
      .setContentTitle(if (sessions == 1) "1 session connected" else "$sessions sessions connected")
      .setContentText(text.ifEmpty { "Tap to return to Citadel" })
      .setContentIntent(open)
      .addAction(R.drawable.ic_stat_citadel, "Disconnect", close)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setShowWhen(false)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .build()
    val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
    ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, type)
    if (sessions <= 0) {
      ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
      stopSelf()
      return START_NOT_STICKY
    }
    val lock = wake ?: getSystemService(PowerManager::class.java)
      .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "citadel:sessions")
      .apply { setReferenceCounted(false) }
    lock.acquire(WAKE_MS)
    wake = lock
    handler.removeCallbacks(refresh)
    handler.postDelayed(refresh, WAKE_MS / 2)
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    handler.removeCallbacks(refresh)
    handler.removeCallbacks(forceStop)
    wake?.let { if (it.isHeld) it.release() }
    wake = null
    super.onDestroy()
  }

  // The app closes its sessions and then stops this service through keepAlive. If the app cannot be
  // reached, drop the notification anyway so the button never looks dead.
  private fun disconnect() {
    val notify = onDisconnect
    val sent = try {
      notify?.invoke()
      notify != null
    } catch (e: Exception) {
      false
    }
    if (sent) handler.postDelayed(forceStop, DISCONNECT_GRACE_MS) else forceStop.run()
  }

  private fun ensureChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = getSystemService(NotificationManager::class.java)
    if (manager.getNotificationChannel(CHANNEL) != null) return
    val channel = NotificationChannel(CHANNEL, "Active sessions", NotificationManager.IMPORTANCE_LOW)
    channel.description = "Shown while SSH sessions stay connected in the background"
    channel.setShowBadge(false)
    manager.createNotificationChannel(channel)
  }

  companion object {
    const val SESSIONS = "sessions"
    const val TEXT = "text"
    private const val ACTION_DISCONNECT = "io.github.sepehrmohseni.citadel.DISCONNECT"
    private const val CHANNEL = "sessions"
    private const val NOTIFICATION_ID = 7
    private const val WAKE_MS = 6L * 60 * 60 * 1000
    private const val DISCONNECT_GRACE_MS = 5_000L

    @Volatile
    var onDisconnect: (() -> Unit)? = null
  }
}
