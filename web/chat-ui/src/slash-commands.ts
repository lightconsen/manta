/**
 * Slash command parser and types for Manta chat UI.
 * Matches OpenClaw-style `/command [args]` syntax.
 */

export type CommandCategory = "session" | "model" | "status" | "agents" | "tools" | "admin";

export type CommandTier = "essential" | "standard" | "power";

export interface CommandDef {
  key: string;
  name: string;
  description: string;
  args?: string;
  category: CommandCategory;
  tier: CommandTier;
  local: boolean;
  requires_admin: boolean;
}

export interface ParsedCommand {
  command: string;
  args: string;
  raw: string;
}

/** Parse a slash command from input text. Returns null if not a command. */
export function parseCommand(text: string): ParsedCommand | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("/")) {
    return null;
  }

  const body = trimmed.slice(1);
  const firstSpace = body.search(/\s/);
  const command = firstSpace === -1 ? body : body.slice(0, firstSpace);
  const args = firstSpace === -1 ? "" : body.slice(firstSpace + 1).trim();

  if (!command) {
    return null;
  }

  return { command: command.toLowerCase(), args, raw: trimmed };
}

/** Check if text contains only a command (no other content). */
export function isCommandOnly(text: string): boolean {
  return parseCommand(text) !== null && !text.trim().slice(1).includes(" ") === false
    ? text.trim().startsWith("/") && !text.trim().slice(1).includes(" ")
    : parseCommand(text) !== null;
}

/** Default built-in commands (fallback before backend catalog loads). */
export const FALLBACK_COMMANDS: CommandDef[] = [
  { key: "new", name: "new", description: "Start a new session", args: "[model]", category: "session", tier: "essential", local: true, requires_admin: false },
  { key: "reset", name: "reset", description: "Reset the current session", args: "[soft|hard]", category: "session", tier: "essential", local: false, requires_admin: false },
  { key: "stop", name: "stop", description: "Abort the current run", category: "session", tier: "essential", local: false, requires_admin: false },
  { key: "compact", name: "compact", description: "Compact the session context", args: "[instructions]", category: "session", tier: "standard", local: false, requires_admin: false },
  { key: "clear", name: "clear", description: "Clear chat history", category: "session", tier: "standard", local: true, requires_admin: false },
  { key: "model", name: "model", description: "Show or switch the active model", args: "[name|#|status]", category: "model", tier: "standard", local: false, requires_admin: false },
  { key: "think", name: "think", description: "Set thinking level", args: "<level>", category: "model", tier: "standard", local: false, requires_admin: false },
  { key: "verbose", name: "verbose", description: "Toggle verbose output", args: "on|off|full", category: "model", tier: "standard", local: false, requires_admin: false },
  { key: "fast", name: "fast", description: "Show or set fast mode", args: "[on|off|status]", category: "model", tier: "standard", local: false, requires_admin: false },
  { key: "help", name: "help", description: "Show help summary", category: "status", tier: "essential", local: false, requires_admin: false },
  { key: "commands", name: "commands", description: "Show full command catalog", category: "status", tier: "essential", local: false, requires_admin: false },
  { key: "status", name: "status", description: "Show runtime status", category: "status", tier: "essential", local: false, requires_admin: false },
  { key: "tools", name: "tools", description: "Show available tools", args: "[compact|verbose]", category: "status", tier: "standard", local: false, requires_admin: false },
  { key: "whoami", name: "whoami", description: "Show your sender ID", category: "status", tier: "essential", local: false, requires_admin: false },
  { key: "usage", name: "usage", description: "Show usage statistics", args: "[off|tokens|full|cost]", category: "status", tier: "standard", local: false, requires_admin: false },
  { key: "subagents", name: "subagents", description: "Manage sub-agents", args: "list|kill|log|info|send|steer|spawn", category: "agents", tier: "standard", local: false, requires_admin: false },
  { key: "acp", name: "acp", description: "Manage ACP sessions", args: "spawn|cancel|steer|close|sessions|status|...", category: "agents", tier: "standard", local: false, requires_admin: false },
  { key: "kill", name: "kill", description: "Abort sub-agent runs", args: "<id|#|all>", category: "agents", tier: "standard", local: false, requires_admin: false },
  { key: "steer", name: "steer", description: "Send steering to a sub-agent", args: "<id> <message>", category: "agents", tier: "standard", local: false, requires_admin: false },
  { key: "config", name: "config", description: "Read or write config", args: "show|get|set|unset", category: "admin", tier: "power", local: false, requires_admin: true },
  { key: "plugins", name: "plugins", description: "Inspect or toggle plugins", args: "list|install|enable|disable", category: "admin", tier: "power", local: false, requires_admin: true },
  { key: "restart", name: "restart", description: "Restart the gateway", category: "admin", tier: "power", local: false, requires_admin: true },
  { key: "bash", name: "bash", description: "Run a host shell command", args: "<command>", category: "admin", tier: "power", local: false, requires_admin: true },
];

/** Commands executed client-side without RPC. */
export const LOCAL_COMMANDS = new Set(["new", "clear"]);

/** Commands that work as inline shortcuts (embedded in normal messages). */
export const INLINE_SHORTCUTS = new Set(["help", "commands", "status", "whoami"]);

/** Find command def by name or key. */
export function findCommand(name: string, catalog: CommandDef[] = FALLBACK_COMMANDS): CommandDef | null {
  const normalized = name.toLowerCase().replace(/^\//, "");
  return catalog.find((c) => c.key === normalized || c.name === normalized) || null;
}

/** Get completions matching a filter string. */
export function getCommandCompletions(
  filter: string,
  catalog: CommandDef[] = FALLBACK_COMMANDS,
  options?: { showAll?: boolean }
): CommandDef[] {
  const lower = filter.toLowerCase();
  const showAll = options?.showAll ?? false;

  let results = lower
    ? catalog.filter(
        (c) =>
          c.name.startsWith(lower) ||
          c.description.toLowerCase().includes(lower)
      )
    : [...catalog];

  if (!lower && !showAll) {
    results = results.filter((c) => c.tier !== "power");
  }

  const tierOrder = { essential: 0, standard: 1, power: 2 };
  const catOrder = ["session", "model", "status", "agents", "tools", "admin"] as const;

  return results.sort((a, b) => {
    const aTier = tierOrder[a.tier] ?? 1;
    const bTier = tierOrder[b.tier] ?? 1;
    if (aTier !== bTier) return aTier - bTier;
    const aCat = catOrder.indexOf(a.category);
    const bCat = catOrder.indexOf(b.category);
    if (aCat !== bCat) return aCat - bCat;
    return a.name.localeCompare(b.name);
  });
}
