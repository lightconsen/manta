package net.syscity.desktop

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

/**
 * Keeps the app process alive so long-running agent turns (minutes of LLM
 * streaming + tool loops) are not killed when the user switches apps.
 *
 * v1 scopes the service to the gateway lifetime (app foreground → service
 * up): the process hosts the whole agent runtime, so any backgrounded moment
 * can carry an in-flight run. Termux-style always-on while the app is open.
 * A later refinement can scope it to active runs via a gateway→host signal.
 */
class AgentRuntimeService : Service() {
  companion object {
    private const val CHANNEL_ID = "agent_runtime"
    private const val NOTIFICATION_ID = 1

    fun start(context: Context) {
      val intent = Intent(context, AgentRuntimeService::class.java)
      context.startForegroundService(intent)
    }

    fun stop(context: Context) {
      context.stopService(Intent(context, AgentRuntimeService::class.java))
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val manager = getSystemService(NotificationManager::class.java)
    val channel = NotificationChannel(
      CHANNEL_ID,
      "Agent runtime",
      NotificationManager.IMPORTANCE_MIN, // silent, no badge
    )
    manager.createNotificationChannel(channel)

    val openApp = PendingIntent.getActivity(
      this,
      0,
      Intent(this, MainActivity::class.java),
      PendingIntent.FLAG_IMMUTABLE,
    )
    val notification = NotificationCompat.Builder(this, CHANNEL_ID)
      .setContentTitle("Syscity is running")
      .setContentText("Agent runtime active — tap to return")
      .setSmallIcon(R.mipmap.ic_launcher)
      .setContentIntent(openApp)
      .setOngoing(true)
      .build()

    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      startForeground(
        NOTIFICATION_ID,
        notification,
        ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
      )
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
    return START_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null
}
