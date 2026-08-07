import { useState, useEffect } from "react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { Input } from "@/components/ui/Input";

const PROVIDER_LOGOS: Record<string, string> = {
  openai: "/assets/providers/openai.svg",
  deepseek: "/assets/providers/deepseek.svg",
  ollama: "/assets/providers/ollama.svg",
  qwen: "/assets/providers/qwen.svg",
  kimi: "/assets/providers/moonshot.svg",
  anthropic: "/assets/providers/anthropic.svg",
  azure: "/assets/providers/azure.svg",
  gemini: "/assets/providers/gemini.svg",
  glm: "/assets/providers/chatglm.svg",
  minimax: "/assets/providers/minimax.svg",
};

type ModelPreset = {
  name: string;
  display_name: string;
  base_url?: string;
  models: string[];
  protocol?: "open_ai" | "anthropic" | "gemini";
  needs_api_key?: boolean;
};

interface AddModelFormProps {
  transport: SyscityWebSocketTransport;
  onAdded?: () => void;
}

export function AddModelForm({ transport, onAdded }: AddModelFormProps) {
  const [addModelError, setAddModelError] = useState("");
  const [newModel, setNewModel] = useState({ name: "", provider: "anthropic", model: "", api_key: "", base_url: "" });
  const [modelActionLoading, setModelActionLoading] = useState<string>("");
  const [modelPresets, setModelPresets] = useState<ModelPreset[]>([]);
  const [remoteModels, setRemoteModels] = useState<string[] | null>(null);
  const [remoteModelsSource, setRemoteModelsSource] = useState<"remote" | "static">("static");
  const [fetchingModels, setFetchingModels] = useState(false);
  const [fetchModelsError, setFetchModelsError] = useState("");

  // Load provider presets on mount.
  useEffect(() => {
    transport.listModelPresets().then(setModelPresets).catch(() => {});
  }, [transport]);

  const selectModelProvider = (provider: string) => {
    const preset = modelPresets.find((p) => p.name === provider);
    setNewModel({
      ...newModel,
      provider,
      model: preset?.models[0] || "",
      base_url: preset?.base_url || "",
    });
    setRemoteModels(null);
    setFetchModelsError("");
    // Providers without auth (e.g. Ollama) can fetch immediately.
    if (preset && preset.needs_api_key === false) {
      setFetchingModels(true);
      transport
        .fetchRemoteModels({
          provider,
          base_url: preset.base_url || undefined,
        })
        .then((res) => {
          setFetchingModels(false);
          setRemoteModels(res.models);
          setRemoteModelsSource(res.source);
          if (res.error) setFetchModelsError(res.error);
          if (res.models.length > 0) {
            setNewModel((prev) => ({ ...prev, model: res.models[0] }));
          }
        });
    }
  };

  const handleFetchModels = async () => {
    setFetchModelsError("");
    setRemoteModels(null);
    const preset = modelPresets.find((p) => p.name === newModel.provider);
    setFetchingModels(true);
    const res = await transport.fetchRemoteModels({
      provider: newModel.provider,
      base_url: newModel.base_url.trim() || undefined,
      api_key: newModel.api_key.trim() || undefined,
      protocol: newModel.provider === "custom" ? (preset?.protocol ?? "open_ai") : undefined,
    });
    setFetchingModels(false);
    setRemoteModels(res.models);
    setRemoteModelsSource(res.source);
    if (res.error) setFetchModelsError(res.error);
    if (res.models.length > 0) {
      setNewModel((prev) => ({
        ...prev,
        model: res.models.includes(prev.model) ? prev.model : res.models[0],
      }));
    }
  };

  // Auto-fetch the model list shortly after the API key is entered.
  useEffect(() => {
    const preset = modelPresets.find((p) => p.name === newModel.provider);
    if (!preset || preset.needs_api_key === false) return;
    const key = newModel.api_key.trim();
    if (key.length < 20) return;
    const t = setTimeout(() => {
      if (!fetchingModels) handleFetchModels();
    }, 800);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [newModel.api_key, newModel.provider]);

  const handleAddModel = async () => {
    setAddModelError("");
    if (!newModel.name.trim()) {
      setAddModelError("Model alias is required");
      return;
    }
    if (!newModel.model.trim()) {
      setAddModelError("Model ID is required");
      return;
    }
    setModelActionLoading("add");
    const ok = await transport.addModel({
      name: newModel.name.trim(),
      provider: newModel.provider,
      model: newModel.model.trim(),
      api_key: newModel.api_key.trim() || undefined,
      base_url: newModel.base_url.trim() || undefined,
    });
    if (ok) {
      setNewModel({ name: "", provider: "anthropic", model: "", api_key: "", base_url: "" });
      setRemoteModels(null);
      setFetchModelsError("");
      onAdded?.();
    } else {
      setAddModelError("Failed to add model");
    }
    setModelActionLoading("");
  };

  return (
    <div className="p-4 rounded-lg bg-card space-y-3">
      <div>
        <label className="block text-xs text-secondary mb-1">Provider</label>
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
          {modelPresets.map((p) => {
            const logo = PROVIDER_LOGOS[p.name];
            const selected = newModel.provider === p.name;
            return (
              <button
                key={p.name}
                type="button"
                onClick={() => selectModelProvider(p.name)}
                className={`flex items-center gap-2 px-2 py-1.5 rounded-lg border text-xs transition ${
                  selected
                    ? "border-primary-400 bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400 font-medium"
                    : "border-subtle text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
                }`}
              >
                {logo ? (
                  <img src={logo} alt="" className="w-5 h-5 object-contain shrink-0" />
                ) : (
                  <span className="w-5 h-5 shrink-0 rounded bg-sidebar flex items-center justify-center text-[10px] font-semibold">
                    {p.display_name.charAt(0)}
                  </span>
                )}
                <span className="truncate">{p.display_name}</span>
              </button>
            );
          })}
        </div>
      </div>
      <Input
        label="Base URL"
        type="text"
        value={newModel.base_url}
        onChange={(e) => setNewModel({ ...newModel, base_url: e.target.value })}
        placeholder={modelPresets.find((p) => p.name === newModel.provider)?.base_url || "https://..."}
      />
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <Input
          label="Name"
          type="text"
          value={newModel.name}
          onChange={(e) => setNewModel({ ...newModel, name: e.target.value })}
          placeholder="smart"
        />
        {modelPresets.find((p) => p.name === newModel.provider)?.needs_api_key !== false && (
          <Input
            label="API Key"
            type="password"
            value={newModel.api_key}
            onChange={(e) => setNewModel({ ...newModel, api_key: e.target.value })}
            onBlur={() => {
              if (newModel.api_key.trim() && remoteModels === null && !fetchingModels) {
                handleFetchModels();
              }
            }}
            placeholder="sk-..."
          />
        )}
      </div>
      {(() => {
        const preset = modelPresets.find((p) => p.name === newModel.provider);
        const optionList = remoteModels && remoteModels.length > 0 ? remoteModels : (preset?.models ?? []);
        return (
          <div>
            <div className="flex items-center justify-between mb-1">
              <label className="block text-xs text-secondary">Model</label>
              <button
                type="button"
                onClick={handleFetchModels}
                disabled={fetchingModels}
                className="text-xs text-primary-600 dark:text-primary-400 hover:underline disabled:opacity-50"
              >
                {fetchingModels ? "Fetching..." : "Fetch Models"}
              </button>
            </div>
            {fetchingModels ? (
              <div className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-secondary">
                Loading model list...
              </div>
            ) : optionList.length > 0 ? (
              <select
                value={newModel.model}
                onChange={(e) => setNewModel({ ...newModel, model: e.target.value })}
                className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
              >
                {optionList.map((m) => (
                  <option key={m} value={m}>{m}</option>
                ))}
              </select>
            ) : (
              <Input
                type="text"
                value={newModel.model}
                onChange={(e) => setNewModel({ ...newModel, model: e.target.value })}
                placeholder="model-id"
              />
            )}
            {remoteModelsSource === "static" && remoteModels !== null && (
              <div className="mt-1 text-xs text-secondary">Showing built-in model list (remote fetch unavailable).</div>
            )}
          </div>
        );
      })()}
      {fetchModelsError && (
        <div className="text-xs text-amber-600 dark:text-amber-400">{fetchModelsError}</div>
      )}
      {addModelError && (
        <div className="text-xs text-red-600 dark:text-red-400">{addModelError}</div>
      )}
      <div className="flex justify-end">
        <button
          onClick={handleAddModel}
          disabled={modelActionLoading === "add"}
          className="px-4 py-1.5 rounded-md bg-primary-500 hover:bg-primary-600 disabled:opacity-50 text-white text-xs font-medium transition"
        >
          {modelActionLoading === "add" ? "Adding..." : "Add Model"}
        </button>
      </div>
    </div>
  );
}
