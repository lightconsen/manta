import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: sessions RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.setSessionModel = async function (this: SyscityWebSocketTransport,
    sessionId: string,
    model: string | null
  ): Promise<boolean> {
    try {
      await this.sendRequestAndWait("sessions.set_model", {
        session_id: sessionId,
        model,
      });
      return true;
    } catch {
      return false;
    }
  };
  proto.setSessionPinned = async function (this: SyscityWebSocketTransport,
    sessionId: string,
    pinned: boolean
  ): Promise<boolean> {
    try {
      await this.sendRequestAndWait("sessions.set_pinned", {
        session_id: sessionId,
        pinned,
      });
      return true;
    } catch {
      return false;
    }
  };
  proto.renameSession = async function (this: SyscityWebSocketTransport,sessionId: string, name: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("sessions.rename", { session_id: sessionId, name });
      return true;
    } catch {
      return false;
    }
  };
  proto.respondToAsk = async function (this: SyscityWebSocketTransport,askId: string, response: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("ask.respond", { ask_id: askId, response });
      return true;
    } catch {
      return false;
    }
  };
  proto.vote = async function (this: SyscityWebSocketTransport,
    turnId: string,
    vote: "up" | "down",
    opts?: { input?: string; response?: string; comment?: string }
  ): Promise<boolean> {
    try {
      await this.sendRequestAndWait("feedback.vote", {
        turn_id: turnId,
        vote,
        ...(opts?.comment ? { comment: opts.comment } : {}),
        ...(opts?.input ? { input: opts.input } : {}),
        ...(opts?.response ? { response: opts.response } : {}),
      });
      return true;
    } catch {
      return false;
    }
  };
}
