import { Menu, Sun, Moon, Settings } from "lucide-react";
import { useChatStore } from "@/stores/chatStore";
import { useThemeStore } from "@/stores/themeStore";
import { usePlatform } from "@/hooks/usePlatform";
import { StatusDot } from "@/components/chat/StatusDot";
import { AccountButton } from "@/components/chat/AccountButton";

interface TitlebarProps {
  isMobile: boolean;
  /** Show the hamburger (mobile drawer trigger). */
  showHamburger: boolean;
  onOpenMobileNav: () => void;
  onOpenSettings: () => void;
}

const iconBtnCls =
  "p-1.5 rounded-md hover:bg-black/5 dark:hover:bg-white/5 text-secondary transition";

/**
 * App-wide top bar (hermes-desktop-style shell chrome):
 *
 *   [traffic lights] [left cluster] ...... center drag region ...... [right cluster]
 *
 * - tauri-macos: the native traffic lights overlay the bar (titleBarStyle
 *   Overlay); the root carries data-tauri-drag-region="deep" so the whole bar
 *   drags the window (interactive elements are excluded by Tauri's drag
 *   script) and double-click toggles zoom. Left padding clears the lights.
 * - every other platform (Windows/Linux desktop, browser, mobile): plain
 *   header strip, native titlebar untouched.
 *
 * Hosts the global controls moved out of the sidebar bottom row (network dot,
 * theme, settings, account) and the agent identity strip moved out of the
 * chat header.
 */
export function Titlebar({
  isMobile,
  showHamburger,
  onOpenMobileNav,
  onOpenSettings,
}: TitlebarProps) {
  const platform = usePlatform();
  const networkStatus = useChatStore((s) => s.networkStatus);
  const currentAgent = useChatStore((s) => s.currentAgent);
  const { resolvedTheme, setTheme } = useThemeStore();

  const isMac = platform === "tauri-macos";
  const showSafeAreaTop = platform === "tauri-mobile" || (isMobile && !isMac);

  return (
    <div
      className={`h-11 shrink-0 flex items-center bg-sidebar border-b border-subtle px-2 ${
        isMac ? "pl-[72px]" : ""
      }`}
      style={
        showSafeAreaTop
          ? { paddingTop: "env(safe-area-inset-top)" }
          : undefined
      }
      data-tauri-drag-region={isMac ? "deep" : undefined}
    >
      {/* Left cluster */}
      <div className="flex items-center gap-2 min-w-0">
        {showHamburger && (
          <button
            className="md:hidden p-2 -ml-1 rounded-lg text-secondary hover:bg-black/5 dark:hover:bg-white/5 transition"
            onClick={onOpenMobileNav}
            aria-label="Open navigation"
          >
            <Menu size={18} />
          </button>
        )}
        <img
          src="/syscity.png"
          alt="Syscity"
          className="w-5 h-5 shrink-0"
          draggable={false}
        />
        {currentAgent ? (
          <div className="flex items-center gap-2 text-xs text-secondary min-w-0">
            <span className="text-sm shrink-0" aria-hidden="true">
              {currentAgent.emoji}
            </span>
            <span className="font-medium truncate">
              {currentAgent.display_name}
            </span>
            <span className="text-[10px] text-secondary/50 truncate">
              ({currentAgent.id})
            </span>
          </div>
        ) : (
          <span className="text-sm font-semibold text-primary whitespace-nowrap">
            Syscity
          </span>
        )}
      </div>

      {/* Center: empty drag region */}
      <div className="flex-1 h-full" />

      {/* Right cluster */}
      <div className="flex items-center gap-1 shrink-0">
        <div className="flex items-center gap-1.5 mr-1" title={`Connection: ${networkStatus}`}>
          <StatusDot status={networkStatus} />
        </div>
        <button
          onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
          className={iconBtnCls}
          title="Toggle theme"
          aria-label="Toggle theme"
        >
          {resolvedTheme === "dark" ? (
            <Sun className="w-4 h-4" />
          ) : (
            <Moon className="w-4 h-4" />
          )}
        </button>
        <button onClick={onOpenSettings} className={iconBtnCls} title="Settings" aria-label="Settings">
          <Settings className="w-4 h-4" />
        </button>
        <AccountButton variant="icon" />
      </div>
    </div>
  );
}
