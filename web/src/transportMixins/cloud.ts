import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: cloud RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.getCloudStatus = async function (this: SyscityWebSocketTransport,): Promise<unknown> {
    return this.sendRequestAndWait("cloud.status", {});
  };
  proto.getCloudSubscription = async function (this: SyscityWebSocketTransport,): Promise<unknown> {
    return this.sendRequestAndWait("cloud.subscription", {});
  };
  proto.getCloudUsage = async function (this: SyscityWebSocketTransport,days = 30): Promise<unknown> {
    return this.sendRequestAndWait("cloud.usage", { days });
  };
  proto.submitCloudToken = async function (this: SyscityWebSocketTransport,token: string): Promise<unknown> {
    return this.sendRequestAndWait("cloud.token", { token });
  };
  proto.cloudLogout = async function (this: SyscityWebSocketTransport,): Promise<void> {
    await this.sendRequestAndWait("cloud.logout", {});
  };
}
