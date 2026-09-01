import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: workspace RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.workspaceList = async function (this: SyscityWebSocketTransport,
    agentId: string | undefined,
    path: string
  ): Promise<{
    root: string;
    path: string;
    entries: Array<{
      name: string;
      path: string;
      kind: "dir" | "file";
      size: number;
      modified?: number;
    }>;
  }> {
    const params: Record<string, unknown> = { path };
    if (agentId) params.agent_id = agentId;
    const res = (await this.sendRequestAndWait("workspace.list", params)) as
      | {
          root?: string;
          path?: string;
          entries?: Array<{
            name: string;
            path: string;
            kind: "dir" | "file";
            size: number;
            modified?: number;
          }>;
        }
      | undefined;
    return { root: res?.root ?? "", path: res?.path ?? path, entries: res?.entries ?? [] };
  };
  proto.workspaceRead = async function (this: SyscityWebSocketTransport,
    agentId: string | undefined,
    path: string
  ): Promise<{
    path: string;
    size: number;
    truncated: boolean;
    binary: boolean;
    content?: string;
  }> {
    const params: Record<string, unknown> = { path };
    if (agentId) params.agent_id = agentId;
    const res = (await this.sendRequestAndWait("workspace.read", params)) as
      | {
          size?: number;
          truncated?: boolean;
          binary?: boolean;
          content?: string;
        }
      | undefined;
    return {
      path,
      size: res?.size ?? 0,
      truncated: res?.truncated ?? false,
      binary: res?.binary ?? false,
      content: res?.content,
    };
  };
}
