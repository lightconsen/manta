import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: skills RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.listSkills = async function (this: SyscityWebSocketTransport,): Promise<{ skills: Array<Record<string, unknown>>; count: number }> {
    const res = await this.sendRequestAndWait("skills.list", {}) as { skills: Array<Record<string, unknown>>; count: number } | undefined;
    return res || { skills: [], count: 0 };
  };
  proto.installSkill = async function (this: SyscityWebSocketTransport,name: string, zipBase64: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("skills.install", { name, zip_base64: zipBase64 });
      return true;
    } catch {
      return false;
    }
  };
}
