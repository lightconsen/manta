import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: marketplace RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.getConnectorsCatalog = async function (this: SyscityWebSocketTransport,): Promise<unknown> {
    return this.sendRequestAndWait("connectors.catalog", {});
  };
}
