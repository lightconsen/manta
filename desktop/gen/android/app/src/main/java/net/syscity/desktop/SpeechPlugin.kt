package net.syscity.desktop

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * Native speech-recognition bridge for voice input.
 *
 * Android WebView does not implement the Web Speech API, so the composer voice
 * mode reaches the system `SpeechRecognizer` through this plugin. The web
 * layer drives the conversation loop (restart-after-final, auto-submit); this
 * plugin deliberately stays a single-shot recognizer wrapper with no restart
 * logic of its own.
 *
 * Registered from Rust via `register_android_plugin("net.syscity.desktop",
 * "SpeechPlugin")` in `desktop/src/lib.rs`. JS invokes it as
 * `plugin:speech|<command>`.
 *
 * All SpeechRecognizer calls must happen on the Android main thread.
 */
@TauriPlugin(
  permissions = [
    Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone"),
  ],
)
class SpeechPlugin(activity: Activity) : Plugin(activity) {

  /** The base `Plugin` keeps its activity private; keep our own reference. */
  private val appActivity = activity

  private val mainHandler = Handler(Looper.getMainLooper())

  /** Active recognizer session; null when idle. Main thread only. */
  private var recognizer: SpeechRecognizer? = null

  /** Event sink for the in-flight session; pushed from the listener. */
  private var events: Channel? = null

  /** Args for `startListening`. The `events` channel streams speech events. */
  class StartArgs {
    var lang: String = "zh-CN"
    lateinit var events: Channel
  }

  // ─────────────────────────────────────────────
  // Availability / permission
  // ─────────────────────────────────────────────

  /** `isAvailable` — whether a speech recognition service exists. */
  @Command
  fun isAvailable(invoke: Invoke) {
    val out = JSObject()
    out.put("available", SpeechRecognizer.isRecognitionAvailable(appActivity))
    out.put(
      "on_device",
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
        SpeechRecognizer.isOnDeviceRecognitionAvailable(appActivity),
    )
    invoke.resolve(out)
  }

  /** `requestMicPermission` — ask the user for RECORD_AUDIO. */
  @Command
  fun requestMicPermission(invoke: Invoke) {
    if (getPermissionState("microphone") == PermissionState.GRANTED) {
      resolvePermissionState(invoke, PermissionState.GRANTED)
      return
    }
    requestPermissionForAliases(arrayOf("microphone"), invoke, "onMicPermissionResult")
  }

  @PermissionCallback
  fun onMicPermissionResult(invoke: Invoke) {
    val state = getPermissionState("microphone") ?: PermissionState.DENIED
    resolvePermissionState(invoke, state)
  }

  private fun resolvePermissionState(invoke: Invoke, state: PermissionState) {
    val out = JSObject()
    out.put("granted", state == PermissionState.GRANTED)
    out.put("state", state.toString())
    invoke.resolve(out)
  }

  // ─────────────────────────────────────────────
  // Listening session
  // ─────────────────────────────────────────────

  /** `startListening` — begin one recognition session, streaming events. */
  @Command
  fun startListening(invoke: Invoke) {
    if (getPermissionState("microphone") != PermissionState.GRANTED) {
      invoke.reject("Microphone permission not granted", "PERMISSION_DENIED")
      return
    }
    if (!SpeechRecognizer.isRecognitionAvailable(appActivity)) {
      invoke.reject("No speech recognition service on this device", "NOT_AVAILABLE")
      return
    }
    val args = invoke.parseArgs(StartArgs::class.java)
    mainHandler.post {
      // A stale session may still hold the mic; drop it before starting anew.
      destroyRecognizer()
      events = args.events
      val sr = SpeechRecognizer.createSpeechRecognizer(appActivity)
      recognizer = sr
      sr.setRecognitionListener(SpeechListener())
      val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
        putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
        putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
        putExtra(RecognizerIntent.EXTRA_LANGUAGE, args.lang)
        putExtra(RecognizerIntent.EXTRA_LANGUAGE_PREFERENCE, args.lang)
      }
      runCatching { sr.startListening(intent) }
        .onSuccess {
          val out = JSObject()
          out.put("started", true)
          invoke.resolve(out)
        }
        .onFailure { e ->
          destroyRecognizer()
          invoke.reject(e.message ?: "startListening failed", "START_FAILED")
        }
    }
  }

  /** `stopListening` — end the current session (idempotent). */
  @Command
  fun stopListening(invoke: Invoke) {
    mainHandler.post {
      destroyRecognizer()
      invoke.resolve()
    }
  }

  /** Tear down the recognizer and emit the terminal state. Main thread only. */
  private fun destroyRecognizer() {
    recognizer?.let { sr ->
      runCatching {
        sr.stopListening()
        sr.destroy()
      }
    }
    recognizer = null
    events = null
  }

  private fun pushEvent(type: String, build: (JSObject) -> Unit = {}) {
    val channel = events ?: return
    val out = JSObject()
    out.put("type", type)
    build(out)
    channel.send(out)
  }

  private inner class SpeechListener : RecognitionListener {
    override fun onReadyForSpeech(params: Bundle?) {
      pushEvent("state") { it.put("value", "listening") }
    }

    override fun onBeginningOfSpeech() {}

    override fun onRmsChanged(rmsdB: Float) {}

    override fun onBufferReceived(buffer: ByteArray?) {}

    override fun onEndOfSpeech() {}

    override fun onError(error: Int) {
      pushEvent("error") { it.put("code", errorName(error)) }
      // The session is over from the web layer's perspective; release the mic
      // so a JS-driven restart starts from a clean state.
      mainHandler.post { destroyRecognizer() }
    }

    override fun onResults(results: Bundle?) {
      val text = results
        ?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
        ?.firstOrNull() ?: ""
      pushEvent("final") { it.put("text", text) }
      mainHandler.post { destroyRecognizer() }
    }

    override fun onPartialResults(partialResults: Bundle?) {
      val text = partialResults
        ?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
        ?.firstOrNull() ?: return
      pushEvent("partial") { it.put("text", text) }
    }

    override fun onEvent(eventType: Int, params: Bundle?) {}
  }

  private fun errorName(error: Int): String = when (error) {
    SpeechRecognizer.ERROR_NETWORK_TIMEOUT -> "network_timeout"
    SpeechRecognizer.ERROR_NETWORK -> "network"
    SpeechRecognizer.ERROR_AUDIO -> "audio"
    SpeechRecognizer.ERROR_SERVER -> "server"
    SpeechRecognizer.ERROR_CLIENT -> "client"
    SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> "speech_timeout"
    SpeechRecognizer.ERROR_NO_MATCH -> "no_match"
    SpeechRecognizer.ERROR_RECOGNIZER_BUSY -> "busy"
    SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "permission"
    SpeechRecognizer.ERROR_TOO_MANY_REQUESTS -> "too_many_requests"
    else -> "unknown_$error"
  }
}
