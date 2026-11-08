package com.exerussus.most

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * Служба переднего плана: в ней живёт узел моста, поэтому соседи, чат и свой сервер на связи и при закрытом окне.
 */
class NodeService : Service() {
    private var netCallback: ConnectivityManager.NetworkCallback? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            Thread { NodeHost.stop() }.start()
            ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        val type = if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        ServiceCompat.startForeground(this, NOTIFY_ID, notification(), type)
        val name = intent?.getStringExtra(EXTRA_NAME) ?: ""
        Thread({ NodeHost.start(applicationContext, name) }, "most-start").start()
        watchNetwork()
        return START_STICKY
    }

    override fun onDestroy() {
        netCallback?.let { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) }
        netCallback = null
        super.onDestroy()
    }

    /** Wi-Fi ↔ мобильная сеть: сразу переподключиться к серверу, не дожидаясь таймеров узла. */
    private fun watchNetwork() {
        if (netCallback != null) return
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = NodeHost.networkChanged()
            override fun onLost(network: Network) = NodeHost.networkChanged()
        }
        getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(cb)
        netCallback = cb
    }

    private fun notification(): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val stop = PendingIntent.getService(
            this, 1, Intent(this, NodeService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE
        )
        return NotificationCompat.Builder(this, MostApp.CHANNEL_NODE)
            .setSmallIcon(R.drawable.ic_notify)
            .setContentTitle("Мост на связи")
            .setContentText("Соседи и чаты доходят и при закрытом окне")
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(0, "Выключить", stop)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
    }

    companion object {
        private const val NOTIFY_ID = 1
        private const val EXTRA_NAME = "name"
        private const val ACTION_STOP = "com.exerussus.most.STOP"

        fun start(ctx: Context, name: String = "") {
            ContextCompat.startForegroundService(ctx, Intent(ctx, NodeService::class.java).putExtra(EXTRA_NAME, name))
        }
    }
}
