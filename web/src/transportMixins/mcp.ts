import type { SyscityWebSocketTransport } from "../transportCore";

// Domain mixin: mcp RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.listMcpPresets = async function (this: SyscityWebSocketTransport,): Promise<
    Array<{
      name: string;
      display_name: string;
      description: string;
      logo_url?: string;
      command?: string;
      args: string[];
      url?: string;
      transport: string;
      enabled: boolean;
      auth_type?: string;
      client_id?: string;
      auth_url?: string;
      token_url?: string;
      scopes?: string;
      env: Array<{ name: string; required: boolean; description?: string }>;
    }>
  > {
    try {
      const res = (await this.sendRequestAndWait("mcp.presets", {})) as {
        presets?: Array<{
          name: string;
          display_name: string;
          description: string;
          logo_url?: string;
          command?: string;
          args: string[];
          url?: string;
          transport: string;
          enabled: boolean;
          auth_type?: string;
          client_id?: string;
          auth_url?: string;
          token_url?: string;
          scopes?: string;
          env: Array<{ name: string; required: boolean; description?: string }>;
        }>;
      };
      return res.presets || [];
    } catch {
      return [];
    }
  };
  proto.listMcpServers = async function (this: SyscityWebSocketTransport,): Promise<{
    servers: Array<{
      id: string;
      transport: string;
      command?: string;
      args: string[];
      url?: string;
      auto_connect: boolean;
      connected: boolean;
      env_configured?: boolean;
    }>;
  }> {
    try {
      const res = (await this.sendRequestAndWait("mcp.list", {})) as {
        servers: Array<{
          id: string;
          transport: string;
          command?: string;
          args: string[];
          url?: string;
          auto_connect: boolean;
          connected: boolean;
          env_configured?: boolean;
        }>;
      };
      return res;
    } catch {
      return { servers: [] };
    }
  };
  proto.addMcpServer = async function (this: SyscityWebSocketTransport,payload: {
    id: string;
    transport: string;
    command?: string;
    args?: string[];
    url?: string;
    auth_type?: string;
    client_id?: string;
    auth_url?: string;
    token_url?: string;
    scopes?: string;
    auto_connect?: boolean;
    env?: Record<string, string>;
  }): Promise<{ ok: boolean; error?: string }> {
    try {
      await this.sendRequestAndWait("mcp.add", payload);
      return { ok: true };
    } catch (e) {
      return { ok: false, error: e instanceof Error ? e.message : String(e) };
    }
  };
  proto.removeMcpServer = async function (this: SyscityWebSocketTransport,id: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("mcp.remove", { id });
      return true;
    } catch {
      return false;
    }
  };
  proto.connectMcpServer = async function (this: SyscityWebSocketTransport,id: string): Promise<{ ok: boolean; error?: string; errorCode?: string; authUrl?: string }> {
    try {
      await this.sendRequestAndWait("mcp.connect", { id });
      return { ok: true };
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      // Check if it's an auth_required error with a JSON payload
      try {
        const parsed = JSON.parse(msg);
        if (parsed.auth_url) {
          return { ok: false, errorCode: "MCP_AUTH_REQUIRED", authUrl: parsed.auth_url };
        }
      } catch {
        // Not JSON, continue
      }
      return { ok: false, error: msg };
    }
  };
  proto.disconnectMcpServer = async function (this: SyscityWebSocketTransport,id: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("mcp.disconnect", { id });
      return true;
    } catch {
      return false;
    }
  };
  proto.cancelMcpAuth = async function (this: SyscityWebSocketTransport,serverId: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("mcp.auth_cancel", { server_id: serverId });
      return true;
    } catch {
      return false;
    }
  };
}
