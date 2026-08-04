package net.syscity.desktop

import android.os.Bundle
import android.system.Os
import androidx.activity.enableEdgeToEdge
import java.io.File

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // Point syscity's data root at the app sandbox *before* the Rust runtime
    // initializes: `super.onCreate` loads the native lib and calls the entry
    // point, and `dirs.rs` reads SYSCITY_HOME at first use.
    Os.setenv("SYSCITY_HOME", File(filesDir, "syscity").absolutePath, true)
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Keep the process alive for long agent turns when backgrounded (§4.7).
    AgentRuntimeService.start(this)
  }

  override fun onDestroy() {
    AgentRuntimeService.stop(this)
    super.onDestroy()
  }
}
