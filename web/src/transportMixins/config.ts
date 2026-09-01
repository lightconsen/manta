import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: config RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.getConfig = async function (this: SyscityWebSocketTransport,): Promise<Record<string, unknown>> {
    const res = await this.sendRequestAndWait("config.get", {}) as Record<string, unknown> | undefined;
    return res || {};
  };
  proto.setConfig = async function (this: SyscityWebSocketTransport,path: string, value: unknown): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path, value });
      return true;
    } catch {
      return false;
    }
  };
}
