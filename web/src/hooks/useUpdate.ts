import { useCallback, useEffect, useState } from "react";

/** Latest release info from GET /api/v1/update/status. */
export interface UpdateStatus {
  enabled: boolean;
  current: string;
  latest?: string;
  update_available: boolean;
  embedded: boolean;
}

/** Phase/percent from GET /api/v1/update/progress. */
export interface UpdateProgress {
  phase:
    | "idle"
    | "checking"
    | "downloading"
    | "verifying"
    | "applying"
    | "restarting"
    | "error";
  percent: number;
  error: string | null;
  current: string;
  latest: string | null;
}

/** True when running inside the Tauri desktop webview. */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI__" in window;
}

/** Mobile gateway requires the per-install token as a Bearer credential. */
function authHeaders(): Record<string, string> {
  const gatewayToken = localStorage.getItem("syscity_gateway_token");
  return gatewayToken ? { Authorization: `Bearer ${gatewayToken}` } : {};
}

async function fetchJson<T>(path: string): Promise<T> {
  const res = await fetch(path, { headers: authHeaders() });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as T;
}

/**
 * Drives the online-update flow. Polls `/api/v1/update/status` on a slow
 * cadence (server-side TTL is 6h); while an update is in flight it polls
 * `/api/v1/update/progress` once a second. In the Tauri desktop webview the
 * trigger button routes to the `check_for_updates` command instead of the
 * daemon endpoint (embedded instances refuse binary replacement).
 */
export function useUpdate() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await fetchJson<UpdateStatus>("/api/v1/update/status"));
    } catch {
      // Gateway unreachable — banner stays hidden until the next poll.
    }
  }, []);

  const refreshProgress = useCallback(async () => {
    try {
      const p = await fetchJson<UpdateProgress>("/api/v1/update/progress");
      setProgress(p);
      // The daemon reports idle/error when a run finished; stop polling.
      if (p.phase === "idle" || p.phase === "error") {
        setBusy(false);
        if (p.phase === "error") {
          setError(p.error || "Update failed.");
        }
      }
    } catch {
      // Daemon may be mid-restart; keep polling until it answers again.
    }
  }, []);

  // Slow status poll; also refresh immediately when visibility returns so a
  // freshly-published release appears without a full app reload.
  useEffect(() => {
    refreshStatus();
    const interval = window.setInterval(refreshStatus, 6 * 60 * 1000);
    const onVisible = () => {
      if (document.visibilityState === "visible") refreshStatus();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refreshStatus]);

  // 1s progress poll while an update is in flight.
  useEffect(() => {
    if (!busy) return;
    refreshProgress();
    const interval = window.setInterval(refreshProgress, 1000);
    return () => window.clearInterval(interval);
  }, [busy, refreshProgress]);

  const runUpdate = useCallback(async () => {
    setError(null);
    setMessage(null);

    if (isTauri()) {
      // Desktop webview: hand off to the Tauri updater command. The invoke
      // resolves only when no update was found (or errors); when an update is
      // installed the app restarts and the promise never settles.
      try {
        setBusy(true);
        const { invoke } = await import("@tauri-apps/api/core");
        const result = (await invoke("check_for_updates")) as string;
        setBusy(false);
        setMessage(result === "up-to-date" ? "已是最新" : result);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        setBusy(false);
      }
      return;
    }

    try {
      const res = await fetch("/api/v1/update", {
        method: "POST",
        headers: authHeaders(),
      });
      if (!res.ok) {
        const body = (await res.json().catch(() => null)) as { message?: string } | null;
        setError(body?.message || `Update request failed (HTTP ${res.status}).`);
        return;
      }
      setMessage("Downloading update…");
      setBusy(true);
      refreshProgress();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [refreshProgress]);

  const checkNow = useCallback(async () => {
    setError(null);
    setMessage(null);
    if (isTauri()) {
      await runUpdate();
      return;
    }
    try {
      setStatus(await fetchJson<UpdateStatus>("/api/v1/update/status?refresh=1"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [runUpdate]);

  return { status, progress, busy, error, message, runUpdate, checkNow };
}
