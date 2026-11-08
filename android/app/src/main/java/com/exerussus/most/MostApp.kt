package com.exerussus.most

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager

class MostApp : Application() {
    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_NODE, getString(R.string.channel_node), NotificationManager.IMPORTANCE_LOW).apply {
                setShowBadge(false)
            }
        )
    }

    companion object {
        const val CHANNEL_NODE = "node"
    }
}
