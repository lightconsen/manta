import { type CommandDef, type CommandCategory } from "@/slash-commands";

function categoryIcon(category: CommandCategory): string {
  const map: Record<CommandCategory, string> = {
    session: "🗂️",
    model: "🧠",
    status: "ℹ️",
    agents: "🤖",
    tools: "🛠️",
    admin: "🔒",
  };
  return map[category];
}

interface CommandPaletteProps {
  commands: CommandDef[];
  selectedIndex: number;
  onSelect: (cmd: CommandDef) => void;
}

export function CommandPalette({ commands, selectedIndex, onSelect }: CommandPaletteProps) {
  if (commands.length === 0) return null;
  return (
    <div className="absolute bottom-full left-0 right-0 mb-2 bg-card rounded-xl shadow-xl border border-subtle overflow-hidden z-50">
      <div className="max-h-64 overflow-y-auto">
        {commands.map((cmd, i) => (
          <button
            key={cmd.key}
            type="button"
            onClick={() => onSelect(cmd)}
            className={`w-full text-left px-4 py-2.5 flex items-center gap-3 transition ${
              i === selectedIndex
                ? "bg-primary-50 dark:bg-primary-900/20"
                : "hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
            }`}
          >
            <span className="text-base shrink-0">{categoryIcon(cmd.category)}</span>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-primary">
                  /{cmd.name}
                </span>
                {cmd.args && (
                  <span className="text-xs text-secondary/70">
                    {cmd.args}
                  </span>
                )}
              </div>
              <div className="text-xs text-secondary truncate">
                {cmd.description}
              </div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
