package com.alstroemeria.app

import android.content.Context
import android.net.wifi.WifiManager
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  // Android drops multicast packets unless an app holds this lock; LAN sync discovery (mDNS) needs it.
  // Sync only runs while the app is in the foreground, so the lock is held from onStart to onStop.
  private var multicastLock: WifiManager.MulticastLock? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Android 17 blocks LAN connections and mDNS until the user grants local network access.
    if (Build.VERSION.SDK_INT >= 37 && checkSelfPermission(LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED) {
      requestPermissions(arrayOf(LOCAL_NETWORK), 7)
    }
  }

  override fun onStart() {
    super.onStart()
    val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
    multicastLock = wifi?.createMulticastLock("alstroemeria-sync")?.apply {
      setReferenceCounted(false)
      acquire()
    }
  }

  override fun onStop() {
    multicastLock?.takeIf { it.isHeld }?.release()
    multicastLock = null
    super.onStop()
  }

  private companion object {
    const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
  }
}
