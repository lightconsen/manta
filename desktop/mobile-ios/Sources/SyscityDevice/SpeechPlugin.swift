// Syscity native iOS speech bridge — composer voice input.
//
// iOS WKWebView does not reliably expose the Web Speech API, so voice input
// uses `SFSpeechRecognizer` (the system speech service) through this plugin.
// Registered from Rust via `register_ios_plugin(init_plugin_syscity_speech)`
// in `desktop/src/lib.rs`; JS invokes it as `plugin:speech|<command>`.
//
// The command/event contract mirrors the Android `SpeechPlugin` verbatim so
// the web layer drives both with one code path: single-shot sessions, no
// internal restart loop — the JS engine selector owns the conversation loop.
//
// Event stream (over the `events` Tauri Channel passed to start_listening):
//   { "type": "state",   "value": "listening" }
//   { "type": "partial", "text": "..." }
//   { "type": "final",   "text": "..." }
//   { "type": "error",   "code": "..." }

import AVFoundation
import Foundation
import Speech

import Tauri

class SpeechPlugin: Plugin {

  private struct StartArgs: Decodable {
    var lang: String?
    var events: Channel
  }

  // ── In-flight session state (single session at a time) ──────────────
  private var audioEngine: AVAudioEngine?
  private var recognizer: SFSpeechRecognizer?
  private var request: SFSpeechAudioBufferRecognitionRequest?
  private var task: SFSpeechRecognitionTask?
  private var events: Channel?

  // ── Availability / permission ───────────────────────────────────────

  /// `isAvailable` — whether the speech service can be used at all.
  @objc public func isAvailable(_ invoke: Invoke) throws {
    let r = SFSpeechRecognizer()
    let auth = SFSpeechRecognizer.authorizationStatus()
    var out = JsonObject()
    out["available"] = (r?.isAvailable ?? false) && auth != .restricted && auth != .denied
    out["on_device"] = r?.supportsOnDeviceRecognition ?? false
    invoke.resolve(JsonValue.dictionary(out))
  }

  /// `requestMicPermission` — speech authorization AND mic record permission.
  @objc public func requestMicPermission(_ invoke: Invoke) throws {
    SFSpeechRecognizer.requestAuthorization { speechStatus in
      let speechGranted = speechStatus == .authorized
      if #available(iOS 17.0, *) {
        AVAudioApplication.requestRecordPermission { micGranted in
          self.resolvePermission(invoke, granted: speechGranted && micGranted)
        }
      } else {
        AVAudioSession.sharedInstance().requestRecordPermission { micGranted in
          self.resolvePermission(invoke, granted: speechGranted && micGranted)
        }
      }
    }
  }

  private func resolvePermission(_ invoke: Invoke, granted: Bool) {
    var out = JsonObject()
    out["granted"] = granted
    out["state"] = granted ? "GRANTED" : "DENIED"
    invoke.resolve(JsonValue.dictionary(out))
  }

  // ── Listening session ───────────────────────────────────────────────

  /// `startListening` — begin one recognition session, streaming events.
  @objc public func startListening(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(StartArgs.self)
    guard SFSpeechRecognizer.authorizationStatus() == .authorized else {
      invoke.reject("Speech recognition not authorized", code: "PERMISSION_DENIED")
      return
    }
    let lang = args.lang ?? "zh-CN"
    guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: lang)),
      recognizer.isAvailable
    else {
      invoke.reject("Speech recognizer unavailable for \(lang)", code: "NOT_AVAILABLE")
      return
    }
    DispatchQueue.main.async {
      self.startSession(args: args, recognizer: recognizer, invoke: invoke)
    }
  }

  /// `stopListening` — end the current session (idempotent).
  @objc public func stopListening(_ invoke: Invoke) throws {
    DispatchQueue.main.async {
      self.stopSession()
      invoke.resolve()
    }
  }

  private func startSession(args: StartArgs, recognizer: SFSpeechRecognizer, invoke: Invoke) {
    stopSession()
    self.events = args.events
    self.recognizer = recognizer

    let engine = AVAudioEngine()
    let request = SFSpeechAudioBufferRecognitionRequest()
    request.shouldReportPartialResults = true

    let task = recognizer.recognitionTask(with: request) { [weak self] result, error in
      guard let self = self else { return }
      if let result = result {
        let text = result.bestTranscription.formattedString
        if result.isFinal {
          self.pushEvent("final") { $0["text"] = text }
          DispatchQueue.main.async { self.stopSession() }
        } else {
          self.pushEvent("partial") { $0["text"] = text }
        }
      }
      if let error = error {
        self.pushEvent("error") { $0["code"] = Self.errorCode(error) }
        DispatchQueue.main.async { self.stopSession() }
      }
    }

    let inputNode = engine.inputNode
    let format = inputNode.outputFormat(forBus: 0)
    inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
      request.append(buffer)
    }

    do {
      let session = AVAudioSession.sharedInstance()
      try session.setCategory(.record, mode: .measurement, options: .duckOthers)
      try session.setActive(true, options: .notifyOthersOnDeactivation)
      engine.prepare()
      try engine.start()
    } catch {
      inputNode.removeTap(onBus: 0)
      task.cancel()
      self.events = nil
      invoke.reject("Audio engine failed: \(error.localizedDescription)", code: "START_FAILED")
      return
    }

    self.audioEngine = engine
    self.request = request
    self.task = task
    pushEvent("state") { $0["value"] = "listening" }
    var out = JsonObject()
    out["started"] = true
    invoke.resolve(JsonValue.dictionary(out))
  }

  /// Tear down the recognizer session. Main thread.
  private func stopSession() {
    audioEngine?.stop()
    audioEngine?.inputNode.removeTap(onBus: 0)
    request?.endAudio()
    task?.cancel()
    audioEngine = nil
    request = nil
    task = nil
    recognizer = nil
    events = nil
    try? AVAudioSession.sharedInstance().setActive(
      false, options: .notifyOthersOnDeactivation)
  }

  private func pushEvent(_ type: String, build: (inout JsonObject) -> Void = { _ in }) {
    guard let channel = events else { return }
    var out = JsonObject()
    out["type"] = type
    build(&out)
    channel.send(out)
  }

  /// Map task errors onto the shared code vocabulary the JS engine knows.
  private static func errorCode(_ error: Error) -> String {
    let ns = error as NSError
    // Our own cancellation (stopListening / session reset) is not an error.
    if ns.domain == NSURLErrorDomain && ns.code == NSURLErrorCancelled { return "client" }
    if ns.domain == NSURLErrorDomain { return "network" }
    // kAFAssistant / SFSpeech errors: 1110 = no speech detected.
    if ns.code == 1110 { return "no_match" }
    return "error_\(ns.code)"
  }
}

// ── Entry point ─────────────────────────────────────────────────────────

@_cdecl("init_plugin_syscity_speech")
func initSpeechPlugin() -> Plugin {
  return SpeechPlugin()
}
