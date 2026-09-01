import type { SyscityWebSocketTransport } from "../transportCore";
import type {
  ModelInfo,
} from "../transportTypes";

// Domain mixin: models RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.listModels = async function (this: SyscityWebSocketTransport,): Promise<{ models: ModelInfo[]; default_model: string }> {
    const res = await this.sendRequestAndWait("models.list", {}) as { models: ModelInfo[]; default_model: string } | undefined;
    return res || { models: [], default_model: "" };
  };
  proto.listModelPresets = async function (this: SyscityWebSocketTransport,): Promise<Array<{ name: string; display_name: string; base_url?: string; models: string[]; protocol?: "open_ai" | "anthropic" | "gemini"; needs_api_key?: boolean }>> {
    try {
      const res = (await this.sendRequestAndWait("models.presets", {})) as { presets?: Array<{ name: string; display_name: string; base_url?: string; models: string[]; protocol?: "open_ai" | "anthropic" | "gemini"; needs_api_key?: boolean }> };
      return res.presets || [];
    } catch {
      return [];
    }
  };
  proto.fetchRemoteModels = async function (this: SyscityWebSocketTransport,payload: { provider: string; base_url?: string; api_key?: string; protocol?: "open_ai" | "anthropic" | "gemini" }): Promise<{ models: string[]; source: "remote" | "static"; error?: string }> {
    try {
      const res = (await this.sendRequestAndWait("models.fetch_remote", payload)) as { models?: string[]; source?: "remote" | "static"; error?: string };
      return { models: res.models || [], source: res.source || "static", error: res.error };
    } catch (e) {
      return { models: [], source: "static", error: e instanceof Error ? e.message : "Request failed" };
    }
  };
  proto.addModel = async function (this: SyscityWebSocketTransport,payload: { provider: string; models: string[]; default_model?: string; api_key?: string; base_url?: string }): Promise<{ ok: boolean; error?: string }> {
    try {
      await this.sendRequestAndWait("models.add", payload);
      return { ok: true };
    } catch (e) {
      return { ok: false, error: e instanceof Error ? e.message : String(e) };
    }
  };
  proto.removeModel = async function (this: SyscityWebSocketTransport,modelId: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("models.remove", { model_id: modelId });
      return true;
    } catch {
      return false;
    }
  };
  proto.setDefaultModel = async function (this: SyscityWebSocketTransport,modelId: string): Promise<boolean> {
    try {
      await this.sendRequestAndWait("models.set_default", { model_id: modelId });
      return true;
    } catch {
      return false;
    }
  };
}
