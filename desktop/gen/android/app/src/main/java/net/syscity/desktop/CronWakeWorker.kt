package net.syscity.desktop

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import androidx.work.WorkerParameters
import androidx.work.CoroutineWorker
import androidx.work.Data
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * One-shot WorkManager alarm fired when a cron job comes due while the app
 * is backgrounded or killed (§4.3).
 *
 * Best-effort wake: posts a heads-up notification naming the job, and tries
 * to start the agent runtime service so an in-process due-job run is not
 * starved. When the app was killed the process is (re)spawned by WorkManager;
 * the gateway is not running, so the notification is the honest fallback —
 * on next open the gateway re-arms from jobs.json and runs the due job.
 */
class CronWakeWorker(
  context: Context,
  params: WorkerParameters,
) : CoroutineWorker(context, params) {

  override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
    val jobId = inputData.getString(KEY_JOB_ID)
    if (jobId.isNullOrBlank()) {
      return@withContext Result.failure()
    }
    postNotification(jobId)
    // Starting a foreground service from a background worker is restricted
    // on modern Android; ignore the failure — the notification above is the
    // fallback that always works.
    runCatching { AgentRuntimeService.start(applicationContext) }
    Result.success()
  }

  private fun postNotification(jobId: String) {
    val manager = applicationContext.getSystemService(NotificationManager::class.java)
    val channel = NotificationChannel(
      "agent_notify",
      "Agent notifications",
      NotificationManager.IMPORTANCE_DEFAULT,
    )
    manager.createNotificationChannel(channel)
    val openApp = PendingIntent.getActivity(
      applicationContext,
      0,
      Intent(applicationContext, MainActivity::class.java),
      PendingIntent.FLAG_IMMUTABLE,
    )
    val notification = NotificationCompat.Builder(applicationContext, "agent_notify")
      .setSmallIcon(R.mipmap.ic_launcher)
      .setContentTitle("Syscity: scheduled job due")
      .setContentText("Cron job '$jobId' is ready to run — open to execute")
      .setContentIntent(openApp)
      .setAutoCancel(true)
      .build()
    manager.notify(jobId.hashCode(), notification)
  }

  companion object {
    private const val KEY_JOB_ID = "job_id"

    /** Build the input for a worker instance (kept for clarity/symmetry). */
    @Suppress("unused")
    private fun inputData(jobId: String): Data =
      androidx.work.workDataOf(KEY_JOB_ID to jobId)
  }
}
