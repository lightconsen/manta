import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: shortcuts RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.runShortcut = async function (this: SyscityWebSocketTransport,name: string, input?: string): Promise<{ launched: boolean } | null> {
    try {
      const res = (await this.sendRequestAndWait(
        "device.shortcut.run",
        { name, input: input || null },
        15000
      )) as { launched?: boolean } | undefined;
      return { launched: !!res?.launched };
    } catch {
      return null;
    }
  };
  proto.shortcutResults = async function (this: SyscityWebSocketTransport,): Promise<Array<{ output?: string; at_ms?: number; file?: string }> | null> {
    try {
      const res = (await this.sendRequestAndWait("device.shortcut.results", {})) as
        | { items?: Array<{ output?: string; at_ms?: number; file?: string }> }
        | undefined;
      return res?.items || [];
    } catch {
      return null;
    }
  };
  proto.shortcutInbox = async function (this: SyscityWebSocketTransport,): Promise<Array<{ prompt?: string; at_ms?: number; file?: string }> | null> {
    try {
      const res = (await this.sendRequestAndWait("device.shortcut.inbox", {})) as
        | { items?: Array<{ prompt?: string; at_ms?: number; file?: string }> }
        | undefined;
      return res?.items || [];
    } catch {
      return null;
    }
  };
}
