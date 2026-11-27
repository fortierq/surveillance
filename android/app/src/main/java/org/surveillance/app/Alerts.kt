package org.surveillance.app

import android.Manifest
import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext

const val channelId = "surveillance_alerts"
const val eventExtra = "event_id"

class SurveillanceApp : Application() {
    override fun onCreate() {
        super.onCreate()
        val channel = NotificationChannel(channelId, getString(R.string.notification_channel_name), NotificationManager.IMPORTANCE_HIGH)
        channel.lockscreenVisibility = Notification.VISIBILITY_PRIVATE
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }
}

fun firebaseAvailable(context: Context): Boolean = FirebaseApp.getApps(context).isNotEmpty()

fun notificationsAllowed(context: Context): Boolean =
    (Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) &&
        context.getSystemService(NotificationManager::class.java).areNotificationsEnabled() &&
        context.getSystemService(NotificationManager::class.java).getNotificationChannel(channelId)?.importance != NotificationManager.IMPORTANCE_NONE

fun schedulePushRegistration(context: Context) {
    if (!firebaseAvailable(context) || !notificationsAllowed(context)) return
    val request = OneTimeWorkRequestBuilder<PushRegistration>()
        .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
        .build()
    WorkManager.getInstance(context).enqueueUniqueWork("register_push", ExistingWorkPolicy.REPLACE, request)
}

class PushRegistration(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        if (!firebaseAvailable(applicationContext) || !notificationsAllowed(applicationContext)) {
            return@withContext Result.success()
        }
        val connection = runCatching { ConnectionStore(applicationContext).load() }.getOrNull()
            ?: return@withContext Result.success()
        try {
            val messaging = FirebaseMessaging.getInstance()
            messaging.isAutoInitEnabled = true
            HostApi(connection).registerPush(messaging.token.await())
            Result.success()
        } catch (error: HttpFailure) {
            if (error.status >= 500) Result.retry() else Result.failure()
        } catch (_: IOException) {
            Result.retry()
        }
    }
}

class AlertService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        schedulePushRegistration(this)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val event = eventId(message.data[eventExtra]) ?: return
        if (!notificationsAllowed(this)) return
        val intent = Intent(this, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            .putExtra(eventExtra, event)
        val pending = PendingIntent.getActivity(this, event.hashCode(), intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, channelId)
            .setSmallIcon(android.R.drawable.ic_dialog_alert)
            .setContentTitle(getString(R.string.notification_title))
            .setContentText(getString(R.string.notification_body))
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setContentIntent(pending)
            .setAutoCancel(true)
            .build()
        getSystemService(NotificationManager::class.java).notify(event, 0, notification)
    }
}
