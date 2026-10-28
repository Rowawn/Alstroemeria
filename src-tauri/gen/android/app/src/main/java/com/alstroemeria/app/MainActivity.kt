package com.alstroemeria.app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.webkit.JavascriptInterface
import android.webkit.WebView
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
  }

  // The UI asks for local network access when the Devices tab or pairing opens (SPEC §9), through this bridge.
  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(Bridge(), "AlstAndroid")
  }

  private inner class Bridge {
    /** "granted", "denied" or "unneeded" (before Android 17 there is no such permission). */
    @JavascriptInterface
    fun localNetwork(): String = when {
      Build.VERSION.SDK_INT < 37 -> "unneeded"
      checkSelfPermission(LOCAL_NETWORK) == PackageManager.PERMISSION_GRANTED -> "granted"
      else -> "denied"
    }

    /** Android 17 blocks LAN connections and mDNS until granted. Shows the system prompt if it still can. */
    @JavascriptInterface
    fun requestLocalNetwork() {
      if (Build.VERSION.SDK_INT >= 37 && checkSelfPermission(LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED) {
        runOnUiThread { requestPermissions(arrayOf(LOCAL_NETWORK), 7) }
      }
    }

    /** For when the prompt no longer appears: this app's page in system settings. */
    @JavascriptInterface
    fun openAppSettings() {
      runOnUiThread {
        startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName")))
      }
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
