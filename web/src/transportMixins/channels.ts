import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: channels RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.addChannel = async function (this: SyscityWebSocketTransport,payload: { name: string; channel_type: string; enabled?: boolean; agent_id?: string; credentials?: Record<string, string> }): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.add", value: payload });
      return true;
    } catch {
      return false;
    }
  };
  proto.updateChannel = async function (this: SyscityWebSocketTransport,payload: { name: string; enabled?: boolean; agent_id?: string; credentials?: Record<string, string> }): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.update", value: payload });
      return true;
    } catch {
      return false;
    }
  };
  proto.removeChannel = async function (this: SyscityWebSocketTransport,name: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.remove", value: name });
      return true;
    } catch {
      return false;
    }
  };
  proto.setChannelEnabled = async function (this: SyscityWebSocketTransport,name: string, enabled: boolean): Promise<boolean> {
    try {
      await this.sendRequestAndWait("config.set", { path: "channels.set_enabled", value: { name, enabled } });
      return true;
    } catch {
      return false;
    }
  };
}
