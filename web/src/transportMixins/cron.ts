import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: cron RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.listCrons = async function (this: SyscityWebSocketTransport,): Promise<{ jobs: Array<Record<string, unknown>>; count: number }> {
    const res = await this.sendRequestAndWait("cron.list", {}) as { jobs: Array<Record<string, unknown>>; count: number } | undefined;
    return res || { jobs: [], count: 0 };
  };
}
