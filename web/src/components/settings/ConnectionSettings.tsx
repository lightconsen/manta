import { useCallback, useEffect, useState } from "react";
import { Section } from "@/components/ui/Section";

/**
 * Desktop connection settings: run the local (embedded/reused) gateway, or
 * connect to a remote gateway on another host. Persisted to
 * `~/.syscity/client.toml` and applied on the next app launch.
 *
 * Tauri-only (the browser UI is served by a gateway and has no connection
 * concept). In remote mode the gateway must be listening with
 * `auth_mode = "token"` — see docs/remote-access.md.
 */

interface ConnectionConfig {
  mode: "local" | "remote";
  host: string;
  port: number;
  token?: string | null;
}

const inputCls =
  "w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20";

export function ConnectionSettings() {
  const [config, setConfig] = useState<ConnectionConfig | null>(null);
  const [host, setHost] = useState("127.0.0.1");
  const [port, setPort] = useState(18080);
  const [token, setToken] = useState("");
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const c = await invoke<ConnectionConfig>("get_connection");
        if (cancelled) return;
        setConfig(c);
        setHost(c.host);
        setPort(c.port);
        setToken(c.token ?? "");
      } catch {
        // Command unavailable (mobile build) — show an explanatory note.
        if (!cancelled) setUnavailable(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  if (unavailable) {
    return (
      <div className="space-y-5">
        <Section title="Connection">
          <p className="text-sm text-secondary">
            Remote gateway connection is available on the desktop app. On mobile,
            the app always runs its own embedded gateway.
          </p>
        </Section>
      </div>
    );
  }

  const test = useCallback(async () => {
    setTesting(true);
    setMsg(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const res = await invoke<string>("test_remote_gateway", {
        host,
        port,
        token: token.trim() || null,
      });
      setMsg({ ok: true, text: res });
    } catch (e) {
      setMsg({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setTesting(false);
    }
  }, [host, port, token]);

  const save = useCallback(async () => {
    setSaving(true);
    setMsg(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("save_connection", {
        config: {
          mode: config?.mode ?? "local",
          host,
          port,
          token: token.trim() || null,
        },
      });
      setMsg({ ok: true, text: "Saved — restart the app to apply the new connection." });
    } catch (e) {
      setMsg({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setSaving(false);
    }
  }, [config, host, port, token]);

  if (!config) return null;

  return (
    <div className="space-y-5">
      <Section title="Connection">
        <p className="text-sm text-secondary mb-3">
          Choose how this app talks to a Syscity Gateway: run one locally, or
          connect to a gateway on another host (set up per{" "}
          <code className="text-primary">docs/remote-access.md</code>).
        </p>

        <div className="flex items-center gap-4 mb-4">
          <label className="flex items-center gap-2 text-sm">
            <input
              type="radio"
              checked={config.mode === "local"}
              onChange={() => setConfig({ ...config, mode: "local" })}
            />
            Local (embedded / reuse)
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="radio"
              checked={config.mode === "remote"}
              onChange={() => setConfig({ ...config, mode: "remote" })}
            />
            Remote gateway
          </label>
        </div>

        {config.mode === "remote" && (
          <div className="space-y-3 mb-4">
            <div>
              <label className="text-xs text-secondary block mb-1">Host</label>
              <input
                className={inputCls}
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="192.168.1.10"
              />
            </div>
            <div>
              <label className="text-xs text-secondary block mb-1">Port</label>
              <input
                className={inputCls}
                type="number"
                value={port}
                onChange={(e) => setPort(Number(e.target.value))}
              />
            </div>
            <div>
              <label className="text-xs text-secondary block mb-1">
                Token (the remote gateway's shared token)
              </label>
              <input
                className={inputCls}
                type="password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="leave blank if the gateway is unauthenticated"
              />
            </div>
            <div className="flex items-center gap-2">
              <button
                onClick={test}
                disabled={testing}
                className="px-3 py-1.5 rounded-lg border border-subtle text-sm transition hover:bg-black/[0.03] dark:hover:bg-white/[0.04] disabled:opacity-50"
              >
                {testing ? "Testing…" : "Test connection"}
              </button>
            </div>
          </div>
        )}

        {msg && (
          <p
            className={`text-sm mb-3 ${
              msg.ok ? "text-emerald-600 dark:text-emerald-400" : "text-red-600 dark:text-red-400"
            }`}
          >
            {msg.text}
          </p>
        )}

        <button
          onClick={save}
          disabled={saving}
          className="px-3 py-1.5 rounded-lg bg-primary-600 text-white text-sm transition hover:bg-primary-700 disabled:opacity-50"
        >
          {saving ? "Saving…" : "Save"}
        </button>
      </Section>
    </div>
  );
}
