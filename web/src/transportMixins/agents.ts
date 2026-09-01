import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: agents RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.listAgents = async function (this: SyscityWebSocketTransport,): Promise<{ agents: string[] }> {
    const res = await this.sendRequestAndWait("agents.list", {}) as { agents: string[] } | undefined;
    return res || { agents: [] };
  };
  proto.getAgent = async function (this: SyscityWebSocketTransport,agentId: string): Promise<{
    agent_id: string;
    busy: boolean;
    status: string;
    config: Record<string, unknown> | null;
    personality: Record<string, unknown> | null;
  } | null> {
    const res = await this.sendRequestAndWait("agents.get", { agent_id: agentId }) as {
      agent_id: string;
      busy: boolean;
      status: string;
      config: Record<string, unknown> | null;
      personality: Record<string, unknown> | null;
    } | undefined;
    return res || null;
  };
}
