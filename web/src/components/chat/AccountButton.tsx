import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { User, LogOut } from "lucide-react";
import { cloudLogin, cloudLogout, cloudStatus, type CloudStatus } from "@/lib/cloud";

/**
 * Persistent account/login entry in the sidebar, rendered as a full-width
 * row (like "+ New session"), placed below the logo bar.
 *
 * - cloud disabled → hidden entirely (matches the rest of the UI).
 * - signed out → a `[👤] Sign in` row that kicks off the cloud OAuth flow.
 * - signed in → a `[avatar] name` row that opens an account menu (name/email +
 *   sign out). The menu is rendered via a portal so the sidebar's
 *   `overflow-x-hidden` never clips it, opening below the row.
 */
export function AccountButton() {
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuPos, setMenuPos] = useState<{ top: number; left: number } | null>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    cloudStatus().then(setStatus).catch(() => setStatus(null));
  }, []);

  // Close the menu on outside click.
  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (btnRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      setMenuOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [menuOpen]);

  if (!status?.enabled) return null;

  const user = status.user;
  const display = user?.name ?? user?.email ?? user?.id ?? "";
  const initial = (display[0] ?? "?").toUpperCase();

  const toggleMenu = () => {
    if (menuOpen) {
      setMenuOpen(false);
      return;
    }
    const r = btnRef.current?.getBoundingClientRect();
    if (r) {
      setMenuPos({ top: r.bottom + 8, left: r.left });
    }
    setMenuOpen(true);
  };

  const rowCls =
    "w-full flex items-center gap-2 px-3 py-2 rounded-lg text-sm transition " +
    "text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04]";

  if (!status.logged_in) {
    return (
      <button
        onClick={() => cloudLogin("github")}
        className={rowCls}
        title="Sign in to Syscity Cloud"
        aria-label="Sign in to Syscity Cloud"
      >
        <User className="w-4 h-4 shrink-0" />
        <span>Sign in</span>
      </button>
    );
  }

  return (
    <>
      <button
        ref={btnRef}
        onClick={toggleMenu}
        className={`${rowCls} text-primary`}
        title={display || "Account"}
        aria-label="Account"
      >
        <span className="w-6 h-6 shrink-0 rounded-full bg-primary-500 text-white text-xs font-semibold flex items-center justify-center">
          {initial}
        </span>
        <span className="truncate flex-1 text-left">{display || "Account"}</span>
      </button>
      {menuOpen &&
        menuPos &&
        createPortal(
          <div
            ref={menuRef}
            className="fixed z-50 w-56 rounded-lg border border-subtle bg-card shadow-xl p-2 text-xs"
            style={{ top: menuPos.top, left: menuPos.left }}
          >
            <div className="px-2 py-1.5 text-primary font-medium truncate">
              {display || "Signed in"}
            </div>
            {user?.email && (
              <div className="px-2 pb-1.5 text-secondary truncate">{user.email}</div>
            )}
            <div className="my-1 border-t border-subtle" />
            <button
              type="button"
              onClick={() => {
                setMenuOpen(false);
                void cloudLogout();
                setStatus({ ...status, logged_in: false, user: null });
              }}
              className="w-full flex items-center gap-2 px-2 py-1.5 rounded-md text-secondary hover:bg-black/5 dark:hover:bg-white/5 hover:text-primary transition"
            >
              <LogOut size={12} /> Sign out
            </button>
          </div>,
          document.body,
        )}
    </>
  );
}
