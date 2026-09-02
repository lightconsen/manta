import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: approvals RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.respondToApproval = async function (
    this: SyscityWebSocketTransport,
    approvalId: string,
    decision: "approve" | "deny",
    reason?: string,
  ): Promise<boolean> {
    const method = decision === "approve" ? "approvals.approve" : "approvals.deny";
    const payload = decision === "approve"
      ? { id: approvalId }
      : { id: approvalId, reason: reason || "Denied by operator" };
    const res = await this.sendRequestAndWait(method, payload) as { error?: string } | undefined;
    return !res?.error;
  };
}
