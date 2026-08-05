// Syscity Shortcuts/AppIntents bridge (mobile-migration §4.6).
//
// The 4.6 result channel is Syscity's OWN AppIntent: a user builds a Shortcut
// whose final step is "Save Syscity Output" (`SyscityOutputIntent`). When the
// Shortcut runs, iOS invokes the intent in Syscity's process and `perform()`
// writes `{output, at_ms}` into `<SYSCITY_HOME>/shortcuts/`. The Rust gateway
// picks it up through the `shortcutResults` bridge command — no external
// dependency, and the same sandbox the rest of the app uses.
//
// `AskSyscityIntent` is the reverse channel: Siri / a Shortcuts automation can
// send a prompt into `<SYSCITY_HOME>/shortcuts/inbox/` for the agent to read
// via `shortcutInbox`.
//
// Both intents need iOS 16+; the app target deploys lower, so every type here
// is `@available(iOS 16.0, *)` and the module never touches these symbols on
// older OSes.

import AppIntents
import Foundation

@available(iOS 16.0, *)
struct SyscityOutputIntent: AppIntent {
  static var title: LocalizedStringResource = "Save Syscity Output"
  static var description = IntentDescription(
    "Return text from a Shortcut into Syscity so the agent can read it.")

  @Parameter(title: "Output", description: "The text to hand back to Syscity.")
  var output: String

  func perform() async throws -> some IntentResult {
    try ShortcutsBridge.write(dir: "shortcuts", key: "output", value: output)
    return .result()
  }
}

@available(iOS 16.0, *)
struct AskSyscityIntent: AppIntent {
  static var title: LocalizedStringResource = "Ask Syscity"
  static var description = IntentDescription(
    "Send a prompt into Syscity for the agent to answer later.")

  @Parameter(title: "Prompt", description: "The question or task for the Syscity agent.")
  var prompt: String

  func perform() async throws -> some IntentResult {
    try ShortcutsBridge.write(dir: "shortcuts/inbox", key: "prompt", value: prompt)
    return .result()
  }
}

/// Shared sandbox writer for the two intents.
@available(iOS 16.0, *)
enum ShortcutsBridge {
  /// `<SYSCITY_HOME>` (mirrors `DevicePlugin.syscityHome`; the two must agree).
  static var home: String {
    if let home = ProcessInfo.processInfo.environment["SYSCITY_HOME"], !home.isEmpty {
      return home
    }
    let docs =
      NSSearchPathForDirectoriesInDomains(.documentDirectory, .userDomainMask, true).first
      ?? NSTemporaryDirectory()
    return (docs as NSString).appendingPathComponent("syscity")
  }

  /// Append `{<key>: value, at_ms: now}` as a JSON file into the given dir.
  static func write(dir: String, key: String, value: String) throws {
    let base = (home as NSString).appendingPathComponent(dir)
    try FileManager.default.createDirectory(atPath: base, withIntermediateDirectories: true)
    let atMs = Int(Date().timeIntervalSince1970 * 1000)
    let name = "\(key)_\(atMs).json"
    let payload: [String: Any] = [key: value, "at_ms": atMs]
    let data = try JSONSerialization.data(withJSONObject: payload)
    try data.write(to: URL(fileURLWithPath: (base as NSString).appendingPathComponent(name)))
  }
}
