import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: local knowledge-base RPC method implementations (installed on
// the prototype by the facade `SyscityWebSocketTransport.ts`). Signatures are
// merged onto the class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.listKbCollections = async function (
    this: SyscityWebSocketTransport
  ): Promise<{ configured: boolean; reason: string | null; collections: Array<Record<string, unknown>> }> {
    const res = (await this.sendRequestAndWait("kb.collections", {})) as
      | { configured: boolean; reason: string | null; collections: Array<Record<string, unknown>> }
      | undefined;
    return res || { configured: false, reason: null, collections: [] };
  };
  proto.listKbDocs = async function (
    this: SyscityWebSocketTransport,
    collection: string
  ): Promise<{ collection: string; docs: Array<Record<string, unknown>> }> {
    const res = (await this.sendRequestAndWait("kb.docs", { collection })) as
      | { collection: string; docs: Array<Record<string, unknown>> }
      | undefined;
    return res || { collection, docs: [] };
  };
  proto.ingestKbDoc = async function (
    this: SyscityWebSocketTransport,
    agentId: string,
    filename: string,
    contentBase64: string
  ): Promise<Record<string, unknown>> {
    return (await this.sendRequestAndWait("kb.ingest", {
      agent_id: agentId,
      filename,
      content_base64: contentBase64,
    })) as Record<string, unknown>;
  };
  proto.deleteKbDoc = async function (
    this: SyscityWebSocketTransport,
    collection: string,
    docId: string
  ): Promise<{ collection: string; doc_id: string; chunks_deleted: number }> {
    return (await this.sendRequestAndWait("kb.delete_doc", { collection, doc_id: docId })) as {
      collection: string;
      doc_id: string;
      chunks_deleted: number;
    };
  };
}
