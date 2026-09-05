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
  // Cloud knowledge bases — thin passthroughs to the cloud server.
  proto.cloudKbList = async function (this: SyscityWebSocketTransport,): Promise<unknown> {
    return this.sendRequestAndWait("cloud.kb.list", {});
  };
  proto.cloudKbCreate = async function (this: SyscityWebSocketTransport, name: string): Promise<unknown> {
    return this.sendRequestAndWait("cloud.kb.create", { name });
  };
  proto.cloudKbDelete = async function (this: SyscityWebSocketTransport, kbId: string): Promise<{ ok: boolean }> {
    return (await this.sendRequestAndWait("cloud.kb.delete", { kb_id: kbId })) as { ok: boolean };
  };
  proto.cloudKbUpload = async function (
    this: SyscityWebSocketTransport,
    kbId: string,
    filename: string,
    contentBase64: string,
    mime?: string
  ): Promise<unknown> {
    return this.sendRequestAndWait("cloud.kb.upload", { kb_id: kbId, filename, content_base64: contentBase64, mime });
  };
  proto.cloudKbQuery = async function (
    this: SyscityWebSocketTransport,
    kbId: string,
    query: string,
    topK = 5
  ): Promise<unknown> {
    return this.sendRequestAndWait("cloud.kb.query", { kb_id: kbId, query, top_k: topK });
  };
}
