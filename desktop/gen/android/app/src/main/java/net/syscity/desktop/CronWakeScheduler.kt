package net.syscity.desktop

import android.content.Context
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.workDataOf
import java.util.concurrent.TimeUnit

/**
 * Mirrors the Rust cron scheduler's schedule into WorkManager one-shot
 * alarms (mobile-migration §4.3).
 *
 * The gateway sends a full snapshot of `(job_id, next_run_at_ms)` for all
 * enabled jobs whenever the schedule changes (and once at startup). Each
 * snapshot replaces the whole alarm set: cancel everything under the shared
 * tag, then re-enqueue one delayed `CronWakeWorker` per job that still has a
 * future run. Jobs with no next run are simply dropped.
 *
 * Honest limitation: a worker cannot run the cron job itself (the gateway
 * only runs while the process is alive). The worker posts a heads-up
 * notification so the user knows the job is due; when they open the app the
 * gateway re-arms from jobs.json and runs due jobs.
 */
object CronWakeScheduler {

  private const val TAG = "syscity_cron_wake"
  private const val UNIQUE_PREFIX = "syscity_cron_"

  data class Job(val id: String, val atMs: Long?)

  fun sync(context: Context, jobs: List<Job>) {
    val wm = WorkManager.getInstance(context)
    // Cancel stale alarms (removed / rescheduled / fired jobs) first, then
    // re-arm the full current set.
    wm.cancelAllWorkByTag(TAG)
    val now = System.currentTimeMillis()
    for (job in jobs) {
      val atMs = job.atMs ?: continue
      val delayMs = atMs - now
      if (delayMs <= 0) continue
      val request = OneTimeWorkRequestBuilder<CronWakeWorker>()
        .setInitialDelay(delayMs, TimeUnit.MILLISECONDS)
        .setInputData(workDataOf("job_id" to job.id))
        .addTag(TAG)
        .build()
      wm.enqueueUniqueWork(UNIQUE_PREFIX + job.id, ExistingWorkPolicy.REPLACE, request)
    }
  }
}
