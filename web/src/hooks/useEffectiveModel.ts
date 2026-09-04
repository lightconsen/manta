import { useEffect, useState } from "react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { useChatStore } from "@/stores/chatStore";

/**
 * Read-only mirror of the ModelSelector's effective-model resolution
 * (session pin -> bound agent's model -> global default) for display
 * surfaces outside the composer (Statusbar).
 *
 * Deliberately not consolidated with ModelSelector: the selector keeps
 * optimistic write state; sharing one implementation would mean building a
 * model-state store. Re-reads on reconnect (models/config only exist after
 * the WS handshake).
 */
export function useEffectiveModel(
  transport: SyscityWebSocketTransport,
): string | null {
  const networkStatus = useChatStore((s) => s.networkStatus);
  const [effective, setEffective] = useState<string | null>(null);

  useEffect(() => {
    if (networkStatus !== "connected") return;
    let cancelled = false;

    const resolve = async () => {
      try {
        const sessionId = transport.getSessionId();
        const [modelsRes, config, sessions] = await Promise.all([
          transport.listModels(),
          transport.getConfig(),
          transport.listSessions(),
        ]);
        if (cancelled) return;
        const agentModels = (config.agent_models as Record<string, string>) || {};
        const session = sessions.find((x) => x.id === sessionId);
        const model =
          session?.model ?? agentModels[session?.agent_id ?? ""] ?? modelsRes.default_model;
        setEffective(model || null);
      } catch {
        /* keep last known value */
      }
    };

    void resolve();
    // Model pins from any client update the session live.
    const unsub = transport.onEvent((evt) => {
      if (evt.event === "session.model_changed") void resolve();
    });
    return () => {
      cancelled = true;
      unsub();
    };
  }, [transport, networkStatus]);

  return effective;
}
