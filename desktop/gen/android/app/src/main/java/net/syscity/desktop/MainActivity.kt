package net.syscity.desktop

import android.os.Build
import android.os.Bundle
import android.system.Os
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.webkit.WebSettingsCompat
import androidx.webkit.WebViewFeature
import java.io.File

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // Point syscity's data root at the app sandbox *before* the Rust runtime
    // initializes: `super.onCreate` loads the native lib and calls the entry
    // point, and `dirs.rs` reads SYSCITY_HOME at first use.
    Os.setenv("SYSCITY_HOME", File(filesDir, "syscity").absolutePath, true)
    // Point the AndroidShellRunner at the APK's extracted native libraries
    // (`jniLibs`) so bundled binaries (sh/toybox) can be exec'd from there.
    Os.setenv("SYSCITY_NATIVE_LIB_DIR", applicationInfo.nativeLibraryDir, true)
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Keep the process alive for long agent turns when backgrounded (§4.7).
    AgentRuntimeService.start(this)
  }

  // Called by Tauri after the WebView is created. Follow the system dark mode
  // so `prefers-color-scheme: dark` reaches the SPA (parity with iOS
  // WKWebView). API 33+ uses the native algorithmic darkening; older APIs use
  // the androidx fallback.
  override fun onWebViewCreate(webView: WebView) {
    if (Build.VERSION.SDK_INT >= 33) {
      webView.settings.isAlgorithmicDarkeningAllowed = true
    } else if (WebViewFeature.isFeatureSupported(WebViewFeature.FORCE_DARK)) {
      WebSettingsCompat.setForceDark(webView.settings, WebSettingsCompat.FORCE_DARK_AUTO)
    }
    super.onWebViewCreate(webView)
  }

  override fun onDestroy() {
    AgentRuntimeService.stop(this)
    super.onDestroy()
  }
}
