import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { User, LogOut, Loader2 } from "lucide-react";
import {
  cloudLoginUrl,
  cloudLogout,
  cloudStatus,
  type CloudStatus,
} from "@/lib/cloud";

const LOGIN_TIMEOUT_MS = 60_000;
const POLL_MS = 1_500;

/**
 * Persistent account/login entry in the sidebar, rendered as a full-width
 * row (like "+ New session"), placed below the logo bar.
 *
 * Sign-in opens the cloud OAuth in a **new tab** (popup flow) so the app
 * never navigates away: the row shows "Signing in…" while the popup runs,
 * the popup notifies back via postMessage and the row also polls
 * `/api/v1/status` as the reliable fallback, flipping to the avatar once the
 * session token is stored. Times out (60s) and resets if the user abandons
 * the popup.
 *
 * - cloud disabled → hidden entirely (matches the rest of the UI).
 * - signed out    → `[👤] Sign in` row.
 * - pending       → `[⏳] Signing in…` row.
 * - signed in     → `[avatar] name` row that opens an account menu (name/email
 *   + sign out). The menu is a portal so the sidebar's overflow-x-hidden
 *   never clips it, opening below the row.
 */
export function AccountButton() {
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [loginPending, setLoginPending] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuPos, setMenuPos] = useState<{ top: number; left: number } | null>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const pollTimer = useRef<number | null>(null);
  const pendingSince = useRef(0);
  const pendingRef = useRef(false);

  const stopPolling = useCallback(() => {
    if (pollTimer.current !== null) {
      window.clearInterval(pollTimer.current);
      pollTimer.current = null;
    }
  }, []);

  const setPending = useCallback((v: boolean) => {
    pendingRef.current = v;
    setLoginPending(v);
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await cloudStatus());
    } catch {
      /* keep last known status */
    }
  }, []);

  // Initial status (e.g. already signed in from a previous session).
  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  /** Fetch status; stop waiting once logged in (or on timeout). */
  const checkAndMaybeStop = useCallback(async () => {
    try {
      const s = await cloudStatus();
      setStatus(s);
      if (s?.logged_in) {
        stopPolling();
        setPending(false);
      }
    } catch {
      /* transient — keep polling */
    }
    if (pendingRef.current && Date.now() - pendingSince.current > LOGIN_TIMEOUT_MS) {
      stopPolling();
      setPending(false);
    }
  }, [setPending, stopPolling]);

  // Popup → opener notification: the OAuth flow finished, check right away
  // (polling continues as the reliable fallback).
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      if (e.data?.type !== "syscity:login") return;
      if (!pendingRef.current) return;
      // The callback may be on localhost while the opener is on 127.0.0.1 —
      // accept either for the local dev loop.
      try {
        const u = new URL(e.origin);
        if (!["localhost", "127.0.0.1"].includes(u.hostname)) return;
        if (u.port !== window.location.port) return;
      } catch {
        return;
      }
      void checkAndMaybeStop();
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [checkAndMaybeStop]);

  const beginLogin = useCallback(() => {
    if (pendingRef.current) return;
    setPending(true);
    pendingSince.current = Date.now();
    window.open(cloudLoginUrl("github"), "_blank");
    stopPolling();
    pollTimer.current = window.setInterval(() => {
      void checkAndMaybeStop();
    }, POLL_MS);
  }, [checkAndMaybeStop, setPending, stopPolling]);

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

  if (loginPending) {
    return (
      <button
        disabled
        className={`${rowCls} opacity-70 cursor-default`}
        title="Signing in…"
        aria-label="Signing in…"
      >
        <Loader2 className="w-4 h-4 shrink-0 animate-spin" />
        <span>Signing in…</span>
      </button>
    );
  }

  if (!status.logged_in) {
    return (
      <button
        onClick={beginLogin}
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
