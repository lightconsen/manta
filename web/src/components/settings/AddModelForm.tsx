import { useState, useEffect, useRef } from "react";
import type { ModelInfo, SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { Input } from "@/components/ui/Input";
import { getProviderLogo } from "@/lib/providerLogos";

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
  /** Already-configured models, used to mark added models and enable updates. */
  models?: ModelInfo[];
  /** Current global default model id, used to preselect on update. */
  globalDefaultModel?: string;
  onAdded?: () => void;
}

export function AddModelForm({
  transport,
  models = [],
  globalDefaultModel,
  onAdded,
}: AddModelFormProps) {
  const [addModelError, setAddModelError] = useState("");
  const [providerName, setProviderName] = useState("");
  const [provider, setProvider] = useState("anthropic");
  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [modelActionLoading, setModelActionLoading] = useState<string>("");
  const [modelPresets, setModelPresets] = useState<ModelPreset[]>([]);
  const [remoteModels, setRemoteModels] = useState<string[] | null>(null);
  const [remoteModelsSource, setRemoteModelsSource] = useState<"remote" | "static">("static");
  const [fetchingModels, setFetchingModels] = useState(false);
  const [fetchModelsError, setFetchModelsError] = useState("");
  const [selectedModels, setSelectedModels] = useState<string[]>([]);
  const [defaultModel, setDefaultModel] = useState("");

  // Load provider presets on mount.
  useEffect(() => {
    transport.listModelPresets().then(setModelPresets).catch(() => {});
  }, [transport]);

  // Match the typed provider name against configured providers
  // (case-insensitive: preset buttons fill display names like "DeepSeek" while
  // the config key is "deepseek").
  const providerKey = providerName.trim().toLowerCase();
  const existingProvider = models.find((m) => m.provider.toLowerCase() === providerKey)?.provider || "";
  // First stored model entry for the matched provider, carrying credential metadata.
  const existingConfig = existingProvider
    ? models.find((m) => m.provider === existingProvider)
    : undefined;
  const savedKey = existingConfig?.has_api_key
    ? existingConfig.api_key_masked || "saved"
    : undefined;

  // When the user lands on a provider that is already configured, preselect its
  // current models and default so the form reads as an update. Only repopulate
  // when the matched provider changes, so manual toggles are preserved.
  const lastMatchedRef = useRef("");
  useEffect(() => {
    if (!existingProvider) return;
    if (existingProvider.toLowerCase() === lastMatchedRef.current) return;
    lastMatchedRef.current = existingProvider.toLowerCase();
    const existing = models.filter((m) => m.provider === existingProvider);
    if (existing.length === 0) return;
    const ids = existing.map((m) => m.id);
    setSelectedModels(ids);
    const prefer = globalDefaultModel && ids.includes(globalDefaultModel) ? globalDefaultModel : existing[0].id;
    setDefaultModel(prefer);
    if (existing[0]?.base_url) setBaseUrl(existing[0].base_url);
    setAddModelError("");
  }, [existingProvider, models, globalDefaultModel]);

  const selectModelProvider = (p: string) => {
    const preset = modelPresets.find((x) => x.name === p);
    setProvider(p);
    setProviderName(preset?.display_name || p);
    setBaseUrl(preset?.base_url || "");
    setRemoteModels(null);
    setFetchModelsError("");
    setSelectedModels([]);
    setDefaultModel("");
    // Providers without auth (e.g. Ollama) can fetch immediately.
    if (preset && preset.needs_api_key === false) {
      setFetchingModels(true);
      transport
        .fetchRemoteModels({
          provider: p,
          base_url: preset.base_url || undefined,
        })
        .then((res) => {
          setFetchingModels(false);
          setRemoteModels(res.models);
          setRemoteModelsSource(res.source);
          if (res.error) setFetchModelsError(res.error);
          if (res.models.length > 0) {
            setSelectedModels([res.models[0]]);
            setDefaultModel(res.models[0]);
          }
        });
    }
  };

  const handleFetchModels = async () => {
    setFetchModelsError("");
    setRemoteModels(null);
    const preset = modelPresets.find((p) => p.name === provider);
    setFetchingModels(true);
    const res = await transport.fetchRemoteModels({
      provider,
      base_url: baseUrl.trim() || undefined,
      api_key: apiKey.trim() || undefined,
      protocol: provider === "custom" ? (preset?.protocol ?? "open_ai") : undefined,
    });
    setFetchingModels(false);
    setRemoteModels(res.models);
    setRemoteModelsSource(res.source);
    if (res.error) setFetchModelsError(res.error);
    if (res.models.length > 0) {
      const kept = selectedModels.filter((m) => res.models.includes(m));
      const nextSelected = kept.length > 0 ? kept : [res.models[0]];
      setSelectedModels(nextSelected);
      setDefaultModel((prev) =>
        prev && res.models.includes(prev) ? prev : nextSelected[0]
      );
    }
  };

  // Auto-fetch the model list shortly after the API key is entered.
  useEffect(() => {
    const preset = modelPresets.find((p) => p.name === provider);
    if (!preset || preset.needs_api_key === false) return;
    const key = apiKey.trim();
    if (key.length < 20) return;
    const t = setTimeout(() => {
      if (!fetchingModels) handleFetchModels();
    }, 800);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [apiKey, provider]);

  const toggleModel = (m: string) => {
    const next = selectedModels.includes(m)
      ? selectedModels.filter((x) => x !== m)
      : [...selectedModels, m];
    setSelectedModels(next);
    if (!next.includes(defaultModel)) setDefaultModel(next[0] || "");
  };

  const handleAddModel = async () => {
    setAddModelError("");
    if (!providerName.trim()) {
      setAddModelError("Provider name is required");
      return;
    }
    if (selectedModels.length === 0) {
      setAddModelError("Select at least one model");
      return;
    }
    setModelActionLoading("add");
    // Submit the canonical config key when updating an existing provider so a
    // display-name spelling ("DeepSeek") does not create a duplicate key.
    const submittedProvider = existingProvider || providerName.trim();
    const res = await transport.addModel({
      provider: submittedProvider,
      models: selectedModels,
      default_model: defaultModel || undefined,
      api_key: apiKey.trim() || undefined,
      base_url: baseUrl.trim() || undefined,
    });
    if (res.ok) {
      setProviderName("");
      setProvider("anthropic");
      setApiKey("");
      setBaseUrl("");
      setSelectedModels([]);
      setDefaultModel("");
      setRemoteModels(null);
      setFetchModelsError("");
      lastMatchedRef.current = "";
      onAdded?.();
    } else {
      setAddModelError(res.error || "Failed to add provider");
    }
    setModelActionLoading("");
  };

  const configuredForProvider = (m: string) =>
    models.some((x) => x.id === m && x.provider.toLowerCase() === providerKey);

  return (
    <div className="p-4 rounded-lg bg-card space-y-3">
      <div>
        <label className="block text-xs text-secondary mb-1">Provider</label>
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
          {modelPresets.map((p) => {
            const logo = getProviderLogo(p.name);
            const selected = provider === p.name;
            const alreadyConfigured = models.some(
              (m) => m.provider.toLowerCase() === p.name.toLowerCase()
            );
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
                {alreadyConfigured && (
                  <span className="ml-auto text-[9px] px-1.5 py-0.5 rounded-full bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400 shrink-0">
                    Configured
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <Input
          label="Provider Name"
          type="text"
          value={providerName}
          onChange={(e) => setProviderName(e.target.value)}
          placeholder="DeepSeek"
        />
        {modelPresets.find((p) => p.name === provider)?.needs_api_key !== false && (
          <div>
            <Input
              label="API Key"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              onBlur={() => {
                if (apiKey.trim() && remoteModels === null && !fetchingModels) {
                  handleFetchModels();
                }
              }}
              placeholder={savedKey ? savedKey : "sk-..."}
            />
            {savedKey && (
              <p className="mt-1 text-[11px] text-secondary">
                A key is already saved — leave blank to keep it, or enter a new one to replace it.
              </p>
            )}
          </div>
        )}
      </div>
      <Input
        label="Base URL"
        type="text"
        value={baseUrl}
        onChange={(e) => setBaseUrl(e.target.value)}
        placeholder={modelPresets.find((p) => p.name === provider)?.base_url || "https://..."}
      />
      {existingProvider && (
        <div className="text-xs px-3 py-2 rounded-lg bg-amber-50 dark:bg-amber-900/20 text-amber-700 dark:text-amber-400">
          Provider already configured — submitting updates its models, default, and credentials.
        </div>
      )}
      {(() => {
        const preset = modelPresets.find((p) => p.name === provider);
        const optionList = remoteModels && remoteModels.length > 0 ? remoteModels : (preset?.models ?? []);
        return (
          <div>
            <div className="flex items-center justify-between mb-1">
              <label className="block text-xs text-secondary">Models</label>
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
              <div className="max-h-40 overflow-y-auto rounded-lg border border-subtle bg-card p-2 space-y-1">
                {optionList.map((m) => (
                  <label
                    key={m}
                    className="flex items-center gap-2 px-2 py-1 rounded hover:bg-black/[0.03] dark:hover:bg-white/[0.04] text-sm cursor-pointer"
                  >
                    <input
                      type="checkbox"
                      checked={selectedModels.includes(m)}
                      onChange={() => toggleModel(m)}
                      className="accent-primary-500"
                    />
                    <span className="truncate text-primary">{m}</span>
                    {configuredForProvider(m) && (
                      <span className="ml-auto text-[9px] px-1.5 py-0.5 rounded-full bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400 shrink-0">
                        Configured
                      </span>
                    )}
                  </label>
                ))}
              </div>
            ) : (
              <Input
                type="text"
                value={selectedModels[0] || ""}
                onChange={(e) => {
                  const v = e.target.value;
                  setSelectedModels(v ? [v] : []);
                  setDefaultModel(v);
                }}
                placeholder="model-id"
              />
            )}
            {remoteModelsSource === "static" && remoteModels !== null && (
              <div className="mt-1 text-xs text-secondary">Showing built-in model list (remote fetch unavailable).</div>
            )}
          </div>
        );
      })()}
      {selectedModels.length > 0 && (
        <div>
          <label className="block text-xs text-secondary mb-1">Default Model</label>
          <select
            value={defaultModel}
            onChange={(e) => setDefaultModel(e.target.value)}
            className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
          >
            {selectedModels.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        </div>
      )}
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
          {modelActionLoading === "add"
            ? existingProvider
              ? "Updating..."
              : "Adding..."
            : existingProvider
              ? "Update Provider"
              : "Add Provider"}
        </button>
      </div>
    </div>
  );
}
