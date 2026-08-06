import { useState, useEffect, useRef } from "react";
import { X, Camera, MapPin, Bell, Vibrate, FileUp, Wifi } from "lucide-react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { useThemeStore } from "@/stores/themeStore";

// Types
interface ChannelConfig {
  name: string;
  channel_type: string;
  enabled: boolean;
  agent_id?: string;
  dm_policy?: string;
  require_mention?: boolean;
  has_credentials?: boolean;
}

interface SyscityConfig {
  model?: string;
  model_provider?: string;
  default_agent?: {
    temperature?: number;
    max_tokens?: number;
    max_turns?: number | null;
    max_concurrent_tools?: number;
    system_prompt?: string;
    workspace_only?: boolean;
  };
  heartbeat?: {
    enabled?: boolean;
    interval_seconds?: number;
    active_hours_start?: string;
    active_hours_end?: string;
    max_consecutive_idle?: number;
  };
  channels?: ChannelConfig[];
  search?: {
    provider?: string;
    providers?: string[];
    has_api_key?: boolean;
    keys?: Record<string, string>;
  };
}

const SEARCH_PROVIDERS = [
  { id: "duckduckgo", label: "DuckDuckGo", needsKey: false },
  { id: "tavily", label: "Tavily", needsKey: true },
  { id: "serpapi", label: "SerpAPI", needsKey: true },
  { id: "exa", label: "Exa", needsKey: true },
  { id: "firecrawl", label: "Firecrawl", needsKey: true },
  { id: "bing", label: "Bing", needsKey: true },
  { id: "google", label: "Google", needsKey: true },
  { id: "brave", label: "Brave", needsKey: true },
];

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

const channelCredentialFields: Record<string, Array<{ key: string; label: string; type?: string }>> = {
  telegram: [{ key: "token", label: "Bot Token", type: "password" }],
  discord: [{ key: "token", label: "Bot Token", type: "password" }],
  slack: [{ key: "token", label: "Bot Token", type: "password" }],
  whatsapp: [
    { key: "phone_number_id", label: "Phone Number ID" },
    { key: "access_token", label: "Access Token", type: "password" },
  ],
  qq: [
    { key: "app_id", label: "App ID" },
    { key: "app_secret", label: "App Secret", type: "password" },
    { key: "bot_qq", label: "Bot QQ" },
  ],
  feishu: [
    { key: "app_id", label: "App ID" },
    { key: "app_secret", label: "App Secret", type: "password" },
  ],
  // signal, imessage, webchat, websocket, web_terminal: no credentials needed
};

interface SettingsPanelProps {
  transport: SyscityWebSocketTransport;
  onClose: () => void;
}

export function SettingsPanel({ transport, onClose }: SettingsPanelProps) {
  const [config, setConfig] = useState<SyscityConfig>({});
  const [models, setModels] = useState<Array<{ id: string; name: string; provider: string }>>([]);
  const [agentRegistry, setAgentRegistry] = useState<Array<{ id: string; display_name: string; emoji?: string; is_valid: boolean; has_heartbeat: boolean }>>([]);
  const [crons, setCrons] = useState<Array<Record<string, unknown>>>([]);
  const [skills, setSkills] = useState<Array<Record<string, unknown>>>([]);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState("general");
  const currentTheme = useThemeStore((s) => s.theme);
  const [showAddChannel, setShowAddChannel] = useState(false);
  const [addChannelError, setAddChannelError] = useState("");
  const [newChannel, setNewChannel] = useState({
    name: "",
    channel_type: "telegram",
    enabled: true,
    agent_id: "",
    credentials: {} as Record<string, string>,
  });
  const [channelActionLoading, setChannelActionLoading] = useState<string>("");
  const [showAddModel, setShowAddModel] = useState(false);
  const [addModelError, setAddModelError] = useState("");
  const [newModel, setNewModel] = useState({ name: "", provider: "anthropic", model: "", api_key: "", base_url: "" });
  const [modelActionLoading, setModelActionLoading] = useState<string>("");
  const [modelPresets, setModelPresets] = useState<Array<{ name: string; display_name: string; base_url?: string; models: string[]; protocol?: "open_ai" | "anthropic" | "gemini"; needs_api_key?: boolean }>>([]);
  const [remoteModels, setRemoteModels] = useState<string[] | null>(null);
  const [remoteModelsSource, setRemoteModelsSource] = useState<"remote" | "static">("static");
  const [fetchingModels, setFetchingModels] = useState(false);
  const [fetchModelsError, setFetchModelsError] = useState("");
  const [showAddSkill, setShowAddSkill] = useState(false);
  const [addSkillError, setAddSkillError] = useState("");
  const [newSkillName, setNewSkillName] = useState("");
  const [newSkillZip, setNewSkillZip] = useState<File | null>(null);
  const [skillActionLoading, setSkillActionLoading] = useState<string>("");
  const [logLines, setLogLines] = useState<string[]>([]);
  const [logsSubscribed, setLogsSubscribed] = useState(false);
  const logListRef = useRef<HTMLDivElement>(null);
  const [deviceCaps, setDeviceCaps] = useState<Array<{ id: string; label: string; available: boolean; granted: boolean }> | null>(null);
  const [deviceCapsLoading, setDeviceCapsLoading] = useState(false);
  const [permRequesting, setPermRequesting] = useState<string>("");
  const [adbPort, setAdbPort] = useState("");
  const [adbConnectPort, setAdbConnectPort] = useState("");
  const [adbCode, setAdbCode] = useState("");
  const [adbStatus, setAdbStatus] = useState<{ paired: boolean; devices: Array<{ serial: string; state: string }> } | null>(null);
  const [adbPairing, setAdbPairing] = useState(false);
  const [adbError, setAdbError] = useState("");
  const [shortcutName, setShortcutName] = useState("");
  const [shortcutInput, setShortcutInput] = useState("");
  const [shortcutRunning, setShortcutRunning] = useState(false);
  const [shortcutMsg, setShortcutMsg] = useState("");
  const [shortcutResults, setShortcutResults] = useState<Array<{ output?: string; at_ms?: number; file?: string }>>([]);
  const [shortcutInbox, setShortcutInbox] = useState<Array<{ prompt?: string; at_ms?: number; file?: string }>>([]);
  const [mcpServers, setMcpServers] = useState<Array<{
    id: string;
    transport: string;
    command?: string;
    args: string[];
    url?: string;
    auto_connect: boolean;
    connected: boolean;
  }>>([]);
  const [showAddMcp, setShowAddMcp] = useState(false);
  const [addMcpError, setAddMcpError] = useState("");
  const [newMcp, setNewMcp] = useState({
    id: "",
    transport: "stdio",
    command: "",
    args: "",
    url: "",
    auth_type: "",
    client_id: "",
    auth_url: "",
    token_url: "",
    scopes: "",
    auto_connect: true,
  });
  const [mcpActionLoading, setMcpActionLoading] = useState<string>("");
  const [mcpPresets, setMcpPresets] = useState<Array<{
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
  }>>([]);
  const [authModal, setAuthModal] = useState<{
    serverId: string;
    authUrl: string;
  } | null>(null);
  // Server id whose enable must be rolled back if its pending OAuth
  // authorization fails or is cancelled. Only ever set during the
  // enable-preset flow, so a Cancel/failed auth flips the toggle back off.
  const pendingEnableRevert = useRef<string | null>(null);
  const [envModal, setEnvModal] = useState<{
    preset: {
      name: string;
      display_name: string;
      command?: string;
      args: string[];
      url?: string;
      transport: string;
      auth_type?: string;
      client_id?: string;
      auth_url?: string;
      token_url?: string;
      scopes?: string;
      env: Array<{ name: string; required: boolean; description?: string }>;
    };
    values: Record<string, string>;
    error?: string;
    saving?: boolean;
  } | null>(null);
  const [toast, setToast] = useState<{ message: string; type: "success" | "error" } | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [selectedAgentId, setSelectedAgentId] = useState("");
  const [selectedAgentDetail, setSelectedAgentDetail] = useState<{
    agent_id: string;
    busy: boolean;
    status: string;
    config: Record<string, unknown> | null;
    personality: Record<string, unknown> | null;
  } | null>(null);
  const [agentDetailLoading, setAgentDetailLoading] = useState(false);

  useEffect(() => {
    setLoading(true);
    Promise.all([
      transport.getConfig(),
      transport.listModels(),
      transport.listAgentRegistry(),
      transport.listCrons(),
      transport.listSkills(),
      transport.listModelPresets(),
      transport.listMcpServers(),
      transport.listMcpPresets(),
    ])
      .then(([cfg, mdl, reg, cronRes, skillRes, presetRes, mcpRes, mcpPresetRes]) => {
        setConfig(cfg as SyscityConfig);
        setModels(mdl.models || []);
        const registry = reg || [];
        setAgentRegistry(registry);
        setCrons(cronRes.jobs || []);
        setSkills(skillRes.skills || []);
        setModelPresets(presetRes || []);
        setMcpServers(mcpRes.servers || []);
        setMcpPresets(mcpPresetRes || []);
        // Auto-select default agent or first available
        const toSelect = registry.some((a) => a.id === "default") ? "default" : (registry[0]?.id || "");
        if (toSelect) {
          setSelectedAgentId(toSelect);
          loadAgentDetail(toSelect);
        }
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [transport]);

  // Subscribe/unsubscribe logs based on active tab
  useEffect(() => {
    if (activeTab === "logs" && !logsSubscribed) {
      transport.subscribeLogs();
      setLogsSubscribed(true);
    } else if (activeTab !== "logs" && logsSubscribed) {
      transport.unsubscribeLogs();
      setLogsSubscribed(false);
      setLogLines([]);
    }
    return () => {
      if (logsSubscribed) {
        transport.unsubscribeLogs();
        setLogsSubscribed(false);
      }
    };
  }, [activeTab, logsSubscribed, transport]);

  // Load device capabilities + adb status when the Devices tab opens
  useEffect(() => {
    if (activeTab !== "devices" || !transport.isTauri()) return;
    let cancelled = false;
    (async () => {
      setDeviceCapsLoading(true);
      const caps = await transport.deviceCapabilities();
      if (!cancelled) {
        setDeviceCaps(caps);
        setDeviceCapsLoading(false);
      }
      const st = await transport.adbStatus();
      if (!cancelled) setAdbStatus(st);
    })();
    return () => {
      cancelled = true;
    };
  }, [activeTab, transport]);

  // Listen for log.line events
  useEffect(() => {
    const unsub = transport.onEvent((evt) => {
      if (evt.event === "log.line") {
        const line = (evt.payload?.line as string) || "";
        setLogLines((prev) => [...prev, line]);
      }
    });
    return unsub;
  }, [transport]);

  // Listen for MCP OAuth events
  useEffect(() => {
    const unsub = transport.onEvent(async (evt) => {
      if (evt.event === "mcp.auth_required") {
        const serverId = evt.payload?.server_id as string;
        const authUrl = evt.payload?.auth_url as string;
        if (serverId && authUrl) {
          setAuthModal({ serverId, authUrl });
        }
      }
      if (evt.event === "mcp.auth_complete") {
        const serverId = evt.payload?.server_id as string;
        setAuthModal(null);
        pendingEnableRevert.current = null;
        // Retry connecting — backend now has a stored token
        if (serverId) {
          const result = await transport.connectMcpServer(serverId);
          if (result.ok) {
            showToast(`Authorization complete, ${serverId} connected`, "success");
          }
        }
        setMcpActionLoading("");
        refreshMcp();
        refreshMcpPresets();
      }
      if (evt.event === "mcp.auth_failed") {
        setAuthModal(null);
        setMcpActionLoading("");
        const serverId = evt.payload?.server_id as string;
        // A failed authorization from the enable-preset flow rolls the enable
        // back so the preset does not stay "Enabled". handleCancelAuth covers
        // the explicit Cancel path; this covers failures that arrive without a
        // Cancel (provider callback / token-exchange errors).
        if (serverId && pendingEnableRevert.current === serverId) {
          pendingEnableRevert.current = null;
          try {
            await transport.removeMcpServer(serverId);
            await transport.disconnectMcpServer(serverId);
          } catch {
            /* rollback is best-effort */
          }
        }
        refreshMcp();
        refreshMcpPresets();
      }
    });
    return unsub;
  }, [transport]);

  // Auto-scroll logs to bottom
  useEffect(() => {
    if (logListRef.current) {
      logListRef.current.scrollTop = logListRef.current.scrollHeight;
    }
  }, [logLines]);

  const update = async (path: string, value: unknown) => {
    const ok = await transport.setConfig(path, value);
    if (ok) {
      setConfig((prev) => {
        const next = { ...prev };
        const parts = path.split(".");
        if (parts.length === 1) {
          (next as Record<string, unknown>)[parts[0]] = value as never;
        } else if (parts.length === 2 && next[parts[0] as keyof SyscityConfig]) {
          const section = { ...(next[parts[0] as keyof SyscityConfig] as Record<string, unknown>) };
          section[parts[1]] = value;
          (next as Record<string, unknown>)[parts[0]] = section as never;
        }
        return next;
      });
    }
  };

  const da = config.default_agent || {};
  const hb = config.heartbeat || {};
  const channels = config.channels || [];

  const refreshConfig = async () => {
    try {
      const cfg = await transport.getConfig();
      setConfig(cfg as SyscityConfig);
    } catch {
      /* ignore */
    }
  };

  const handleAddChannel = async () => {
    setAddChannelError("");
    if (!newChannel.name.trim()) {
      setAddChannelError("Channel name is required");
      return;
    }
    const requiredFields = channelCredentialFields[newChannel.channel_type] || [];
    for (const field of requiredFields) {
      if (!newChannel.credentials[field.key]?.trim()) {
        setAddChannelError(`${field.label} is required`);
        return;
      }
    }
    setChannelActionLoading("add");
    const ok = await transport.addChannel({
      name: newChannel.name.trim(),
      channel_type: newChannel.channel_type,
      enabled: newChannel.enabled,
      agent_id: newChannel.agent_id.trim() || undefined,
      credentials: requiredFields.length > 0 ? newChannel.credentials : undefined,
    });
    if (ok) {
      setNewChannel({ name: "", channel_type: "telegram", enabled: true, agent_id: "", credentials: {} });
      setShowAddChannel(false);
      await refreshConfig();
    } else {
      setAddChannelError("Failed to add channel");
    }
    setChannelActionLoading("");
  };

  const handleRemoveChannel = async (name: string) => {
    if (!confirm(`Remove channel "${name}"?`)) return;
    setChannelActionLoading(name);
    await transport.removeChannel(name);
    await refreshConfig();
    setChannelActionLoading("");
  };

  const handleToggleChannel = async (name: string, enabled: boolean) => {
    setChannelActionLoading(name);
    await transport.setChannelEnabled(name, enabled);
    await refreshConfig();
    setChannelActionLoading("");
  };

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
    if (!showAddModel) return;
    const preset = modelPresets.find((p) => p.name === newModel.provider);
    if (!preset || preset.needs_api_key === false) return;
    const key = newModel.api_key.trim();
    if (key.length < 20) return;
    const t = setTimeout(() => {
      if (!fetchingModels) handleFetchModels();
    }, 800);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [newModel.api_key, newModel.provider, showAddModel]);

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
      setShowAddModel(false);
      await refreshModels();
    } else {
      setAddModelError("Failed to add model");
    }
    setModelActionLoading("");
  };

  const handleRemoveModel = async (name: string) => {
    if (!confirm(`Remove model "${name}"?`)) return;
    setModelActionLoading(name);
    await transport.removeModel(name);
    await refreshModels();
    setModelActionLoading("");
  };

  const handleSetDefaultModel = async (name: string) => {
    setModelActionLoading(`default_${name}`);
    const ok = await transport.setDefaultModel(name);
    if (ok) {
      await refreshModels();
      setConfig((prev) => ({ ...prev, model: name }));
    }
    setModelActionLoading("");
  };

  const refreshModels = async () => {
    try {
      const mdl = await transport.listModels();
      setModels(mdl.models || []);
    } catch {
      /* ignore */
    }
  };

  const refreshMcp = async () => {
    try {
      const res = await transport.listMcpServers();
      setMcpServers(res.servers || []);
    } catch {
      /* ignore */
    }
  };

  const refreshMcpPresets = async () => {
    try {
      const presets = await transport.listMcpPresets();
      setMcpPresets(presets);
    } catch {
      /* ignore */
    }
  };

  const handleEnablePreset = async (preset: {
    name: string;
    display_name: string;
    command?: string;
    args: string[];
    url?: string;
    transport: string;
    auth_type?: string;
    client_id?: string;
    auth_url?: string;
    token_url?: string;
    scopes?: string;
    enabled: boolean;
    env: Array<{ name: string; required: boolean; description?: string }>;
  }) => {
    // Presets that need env tokens: collect them first, then validate on save.
    if (preset.env?.length && !preset.enabled) {
      setEnvModal({ preset, values: {} });
      return;
    }
    setMcpActionLoading(preset.name);
    try {
      const res = await transport.addMcpServer({
        id: preset.name,
        transport: preset.transport,
        command: preset.command,
        args: preset.args,
        url: preset.url,
        auth_type: preset.auth_type,
        client_id: preset.client_id,
        auth_url: preset.auth_url,
        token_url: preset.token_url,
        scopes: preset.scopes,
        auto_connect: true,
      });
      if (!res.ok) {
        showToast(`Failed to enable ${preset.display_name}`, "error");
        setMcpActionLoading("");
        return;
      }
      await Promise.all([refreshMcp(), refreshMcpPresets()]);

      // Try connecting — for OAuth servers this may trigger the auth flow
      const result = await transport.connectMcpServer(preset.name);
      if (result.ok) {
        showToast(`${preset.display_name} enabled`, "success");
      } else if (result.errorCode === "MCP_AUTH_REQUIRED" && result.authUrl) {
        // This auth modal came from the enable flow — if it fails or is
        // cancelled, roll the enable back (remove the server config) so the
        // preset toggle returns to disabled.
        pendingEnableRevert.current = preset.name;
        setAuthModal({ serverId: preset.name, authUrl: result.authUrl });
        showToast(`${preset.display_name}: authorization required`, "success");
      } else if (preset.auth_type === "oauth2") {
        // OAuth server but connect returned unexpected error
        showToast(`${preset.display_name}: connect queued, complete auth to connect`, "success");
      } else {
        // For non-OAuth servers, the config was added successfully
        showToast(`${preset.display_name} enabled`, "success");
      }
    } catch {
      showToast(`Failed to enable ${preset.display_name}`, "error");
    }
    setMcpActionLoading("");
  };

  /** Validate + save env tokens for a preset, then enable it. */
  const submitEnv = async () => {
    if (!envModal) return;
    const p = envModal.preset;
    const env: Record<string, string> = {};
    for (const v of p.env) {
      const val = (envModal.values[v.name] ?? "").trim();
      if (v.required && !val) {
        setEnvModal({ ...envModal, error: `${v.name} is required` });
        return;
      }
      if (val) env[v.name] = val;
    }
    setEnvModal({ ...envModal, saving: true, error: undefined });
    const res = await transport.addMcpServer({
      id: p.name,
      transport: p.transport,
      command: p.command,
      args: p.args,
      url: p.url,
      auth_type: p.auth_type,
      client_id: p.client_id,
      auth_url: p.auth_url,
      token_url: p.token_url,
      scopes: p.scopes,
      auto_connect: true,
      env,
    });
    if (!res.ok) {
      // Keep the typed values and the modal open so the user can retry.
      setEnvModal((m) => (m ? { ...m, saving: false, error: res.error || "Failed to enable" } : m));
      return;
    }
    setEnvModal(null);
    await Promise.all([refreshMcp(), refreshMcpPresets()]);
    showToast(`${p.display_name} enabled`, "success");
  };

  const handleCancelAuth = async () => {
    if (authModal) {
      await transport.cancelMcpAuth(authModal.serverId);
      setAuthModal(null);
      setMcpActionLoading("");
      // Roll back the enable: the preset was only marked enabled as part of the
      // now-cancelled authorize flow. Removing the server config flips the
      // preset toggle back to disabled.
      if (pendingEnableRevert.current === authModal.serverId) {
        pendingEnableRevert.current = null;
        try {
          await transport.removeMcpServer(authModal.serverId);
          await transport.disconnectMcpServer(authModal.serverId);
          await Promise.all([refreshMcp(), refreshMcpPresets()]);
        } catch {
          /* rollback is best-effort */
        }
      }
    }
  };

  const handleDisablePreset = async (name: string) => {
    setMcpActionLoading(name);
    try {
      await transport.removeMcpServer(name);
      await transport.disconnectMcpServer(name);
      await Promise.all([refreshMcp(), refreshMcpPresets()]);
      showToast(`${name} disabled`, "success");
    } catch {
      showToast(`Failed to disable ${name}`, "error");
    }
    setMcpActionLoading("");
  };

  const showToast = (message: string, type: "success" | "error") => {
    if (toastTimer.current) clearTimeout(toastTimer.current);
    setToast({ message, type });
    toastTimer.current = setTimeout(() => setToast(null), 3000);
  };

  /* ── Devices tab (mobile §4.1/§4.5) ── */

  /** Runtime permissions that need the Request button (granted via dialog). */
  const needsDevicePermission = (id: string) =>
    id === "camera" || id === "location" || id === "notifications";

  /** Per-capability icons for the Devices tab list. */
  const deviceCapIcon: Record<string, React.ReactNode> = {
    camera: <Camera className="w-4 h-4" />,
    location: <MapPin className="w-4 h-4" />,
    notifications: <Bell className="w-4 h-4" />,
    haptics: <Vibrate className="w-4 h-4" />,
    file_pick: <FileUp className="w-4 h-4" />,
    adb: <Wifi className="w-4 h-4" />,
  };

  const requestDevicePermission = async (perm: string) => {
    setPermRequesting(perm);
    const res = await transport.requestDevicePermission(perm);
    setPermRequesting("");
    if (!res) {
      showToast(`Failed to request ${perm} permission`, "error");
      return;
    }
    // Update the grant state in the list without a full reload.
    setDeviceCaps((caps) =>
      (caps || []).map((c) => (c.id === perm ? { ...c, granted: res.granted } : c))
    );
    showToast(
      res.granted ? `${perm} permission granted` : `${perm} permission denied`,
      res.granted ? "success" : "error"
    );
  };

  const refreshAdbStatus = async () => {
    const st = await transport.adbStatus();
    setAdbStatus(st);
  };

  const pairAdb = async () => {
    const port = parseInt(adbPort, 10);
    if (!port || !adbCode.trim()) {
      setAdbError("Enter the pairing port and code from the wireless-debugging screen");
      return;
    }
    setAdbPairing(true);
    setAdbError("");
    const connectPort = adbConnectPort ? parseInt(adbConnectPort, 10) : undefined;
    const res = await transport.adbPair(port, adbCode.trim(), connectPort);
    setAdbPairing(false);
    if (!res) {
      setAdbError("Pairing failed — is wireless debugging enabled on this phone?");
      return;
    }
    if (res.paired && res.connected) {
      showToast("Paired with wireless debugging", "success");
      setAdbError("");
    } else if (res.paired) {
      setAdbError(res.connectOutput || "Paired, but the adb connect failed");
    } else {
      setAdbError(res.pairOutput || "Pairing failed — check the code and port");
    }
    setAdbStatus({ paired: res.connected, devices: res.devices });
  };

  // iOS Shortcuts / AppIntents bus (§4.6)
  const isIOSDevice = /iPhone|iPad|iPod/.test(navigator.userAgent);

  const runShortcut = async () => {
    if (!shortcutName.trim()) {
      setShortcutMsg("Enter a shortcut name");
      return;
    }
    setShortcutRunning(true);
    setShortcutMsg("");
    const res = await transport.runShortcut(shortcutName.trim(), shortcutInput || undefined);
    setShortcutRunning(false);
    if (!res) {
      setShortcutMsg("Shortcuts are only available in the Syscity iOS app");
    } else if (res.launched) {
      setShortcutMsg(`Launched "${shortcutName.trim()}" in the Shortcuts app`);
    } else {
      setShortcutMsg("Could not launch — is the shortcut name correct?");
    }
  };

  const refreshShortcutResults = async () => {
    const res = await transport.shortcutResults();
    if (res) setShortcutResults(res);
  };

  const refreshShortcutInbox = async () => {
    const res = await transport.shortcutInbox();
    if (res) setShortcutInbox(res);
  };

  const handleAddMcp = async () => {
    setAddMcpError("");
    if (!newMcp.id.trim()) {
      setAddMcpError("Server ID is required");
      return;
    }
    if (newMcp.transport === "stdio" && !newMcp.command.trim()) {
      setAddMcpError("Command is required for stdio transport");
      return;
    }
    if (newMcp.transport !== "stdio" && !newMcp.url.trim()) {
      setAddMcpError("URL is required for SSE/HTTP transport");
      return;
    }
    setMcpActionLoading("add");
    const res = await transport.addMcpServer({
      id: newMcp.id.trim(),
      transport: newMcp.transport,
      command: newMcp.command.trim() || undefined,
      args: newMcp.args.split(",").map((s) => s.trim()).filter(Boolean),
      url: newMcp.url.trim() || undefined,
      auth_type: newMcp.auth_type || undefined,
      client_id: newMcp.client_id.trim() || undefined,
      auth_url: newMcp.auth_url.trim() || undefined,
      token_url: newMcp.token_url.trim() || undefined,
      scopes: newMcp.scopes.trim() || undefined,
      auto_connect: newMcp.auto_connect,
    });
    if (res.ok) {
      setNewMcp({ id: "", transport: "stdio", command: "", args: "", url: "", auth_type: "", client_id: "", auth_url: "", token_url: "", scopes: "", auto_connect: true });
      setShowAddMcp(false);
      await refreshMcp();
    } else {
      setAddMcpError(res.error || "Failed to add MCP server");
    }
    setMcpActionLoading("");
  };

  const handleRemoveMcp = async (id: string) => {
    if (!confirm(`Remove MCP server "${id}"?`)) return;
    setMcpActionLoading(id);
    await transport.removeMcpServer(id);
    await refreshMcp();
    setMcpActionLoading("");
  };

  const handleConnectMcp = async (id: string) => {
    setMcpActionLoading(id);
    await transport.connectMcpServer(id);
    await refreshMcp();
    setMcpActionLoading("");
  };

  const handleDisconnectMcp = async (id: string) => {
    setMcpActionLoading(id);
    await transport.disconnectMcpServer(id);
    await refreshMcp();
    setMcpActionLoading("");
  };

  const handleAddSkill = async () => {
    setAddSkillError("");
    if (!newSkillName.trim()) {
      setAddSkillError("Skill name is required");
      return;
    }
    if (!newSkillZip) {
      setAddSkillError("ZIP file is required");
      return;
    }
    setSkillActionLoading("add");
    try {
      const arrayBuffer = await newSkillZip.arrayBuffer();
      const bytes = new Uint8Array(arrayBuffer);
      let binary = "";
      for (let i = 0; i < bytes.byteLength; i++) {
        binary += String.fromCharCode(bytes[i]);
      }
      const zipBase64 = btoa(binary);
      const ok = await transport.installSkill(newSkillName.trim(), zipBase64);
      if (ok) {
        setNewSkillName("");
        setNewSkillZip(null);
        setShowAddSkill(false);
        await refreshSkills();
      } else {
        setAddSkillError("Failed to install skill");
      }
    } catch {
      setAddSkillError("Failed to read ZIP file");
    }
    setSkillActionLoading("");
  };

  const refreshSkills = async () => {
    try {
      const skillRes = await transport.listSkills();
      setSkills(skillRes.skills || []);
    } catch {
      /* ignore */
    }
  };

  const loadAgentDetail = async (agentId: string) => {
    if (!agentId) {
      setSelectedAgentDetail(null);
      return;
    }
    setAgentDetailLoading(true);
    try {
      const detail = await transport.getAgent(agentId);
      setSelectedAgentDetail(detail);
    } catch {
      setSelectedAgentDetail(null);
    } finally {
      setAgentDetailLoading(false);
    }
  };

  const tabs = [
    { id: "general", label: "General" },
    { id: "models", label: "Models" },
    { id: "agents", label: "Agents" },
    { id: "channels", label: "Channels" },
    { id: "tools", label: "Web Search" },
    { id: "mcp", label: "MCP Servers" },
    { id: "skills", label: "Skills" },
    { id: "jobs", label: "Jobs" },
    { id: "devices", label: "Devices" },
    { id: "logs", label: "Logs" },
  ];

  const tabCls = (id: string) =>
    `px-3 py-1.5 rounded-md text-sm whitespace-nowrap md:w-full md:text-left transition ${
      activeTab === id
        ? "bg-primary-50/70 dark:bg-primary-900/20 text-primary-700 dark:text-primary-400 font-medium"
        : "text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
    }`;

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-page">
      {/* Header */}
      <div className="flex items-center justify-between px-4 md:px-5 py-3 border-b border-subtle shrink-0">
        <h2 className="text-base font-semibold text-primary">Settings</h2>
        <button
          onClick={onClose}
          className="p-1.5 rounded-lg hover:bg-black/5 dark:hover:bg-white/5 text-secondary transition"
          title="Back to chat"
        >
          <X className="w-5 h-5" />
        </button>
      </div>

      {loading ? (
        <div className="flex-1 flex items-center justify-center text-secondary">
          <div className="w-6 h-6 border-2 border-subtle border-t-primary-500 rounded-full animate-spin mb-3 mr-3" />
          Loading configuration...
        </div>
      ) : (
        <div className="flex-1 flex flex-col md:flex-row overflow-hidden">
          {/* Tabs: horizontal strip on mobile, left sidebar on md+ */}
          <div className="flex md:flex-col gap-0.5 overflow-x-auto md:overflow-y-auto md:overflow-x-hidden border-b md:border-b-0 md:border-r border-subtle shrink-0 py-2 md:py-3 px-2 md:w-44">
            {tabs.map((t) => (
              <button key={t.id} onClick={() => setActiveTab(t.id)} className={`${tabCls(t.id)} shrink-0`}>
                {t.label}
              </button>
            ))}
          </div>

          {/* Right content */}
          <div className="flex-1 overflow-y-auto px-4 md:px-5 py-4">
            {activeTab === "general" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Gateway</h3>
                  <div className="space-y-2">
                    {(() => {
                      const si = transport.getServerInfo();
                      return (
                        <>
                          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-0 px-3 py-2 rounded-lg bg-card">
                            <span className="text-sm text-secondary">URL</span>
                            <span className="text-sm text-primary font-mono break-all sm:text-right">{transport.getGatewayUrl() || "—"}</span>
                          </div>
                          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-0 px-3 py-2 rounded-lg bg-card">
                            <span className="text-sm text-secondary">Version</span>
                            <span className="text-sm text-primary font-mono break-all sm:text-right">{si.version || "—"}</span>
                          </div>
                          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-0 px-3 py-2 rounded-lg bg-card">
                            <span className="text-sm text-secondary">Connection</span>
                            <span className="text-sm text-primary font-mono break-all sm:text-right">{si.conn_id || "—"}</span>
                          </div>
                          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-0 px-3 py-2 rounded-lg bg-card">
                            <span className="text-sm text-secondary">Features</span>
                            <span className="text-sm text-primary break-all sm:text-right">{(si.features || []).join(", ") || "—"}</span>
                          </div>
                          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-0 px-3 py-2 rounded-lg bg-card">
                            <span className="text-sm text-secondary">Auth Mode</span>
                            <span className="text-sm text-primary font-mono break-all capitalize sm:text-right">{String((config as Record<string, unknown>).auth_mode || "—")}</span>
                          </div>
                          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-0 px-3 py-2 rounded-lg bg-card">
                            <span className="text-sm text-secondary">Scopes</span>
                            <span className="text-sm text-primary break-all sm:text-right">{(si.scopes_granted || []).join(", ") || "—"}</span>
                          </div>
                        </>
                      );
                    })()}
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Model & Provider</h3>
                  <div className="space-y-3">
                    <div>
                      <label className="block text-sm text-secondary mb-1">Default Model</label>
                      <select value={config.model || ""} onChange={(e) => update("model", e.target.value)} className="w-full rounded-lg border border-subtle bg-card px-3 py-2 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20">
                        {models.map((m) => <option key={m.id} value={m.id}>{m.name} ({m.provider})</option>)}
                      </select>
                    </div>
                    <div>
                      <label className="block text-sm text-secondary mb-1">Provider</label>
                      <input type="text" value={config.model_provider || ""} readOnly className="w-full rounded-lg border border-subtle bg-sidebar px-3 py-2 text-sm text-secondary cursor-not-allowed" />
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Appearance</h3>
                  <div className="space-y-3">
                    <div>
                      <label className="block text-sm text-secondary mb-1">Theme Mode</label>
                      <div className="flex gap-2">
                        {(["system", "light", "dark"] as const).map((m) => (
                          <button
                            key={m}
                            onClick={() => useThemeStore.getState().setTheme(m)}
                            className={`px-3 py-1.5 rounded-lg border text-sm transition capitalize ${
                              currentTheme === m
                                ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400 border-primary-400 font-medium"
                                : "border-subtle text-secondary hover:bg-black/[0.03] dark:hover:bg-white/[0.04]"
                            }`}
                          >
                            {m}
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Heartbeat</h3>
                  <div className="space-y-3">
                    <div className="flex items-center justify-between">
                      <label className="text-sm text-secondary">Enable Heartbeat</label>
                      <button onClick={() => update("heartbeat.enabled", !hb.enabled)} className={`relative inline-flex h-5 w-9 items-center rounded-full transition ${hb.enabled ? "bg-primary-500" : "bg-secondary/30 dark:bg-secondary/20"}`}>
                        <span className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition ${hb.enabled ? "translate-x-4.5" : "translate-x-0.5"}`} />
                      </button>
                    </div>
                    <div>
                      <label className="block text-sm text-secondary mb-1">Interval (seconds)</label>
                      <input type="number" value={hb.interval_seconds ?? 300} onChange={(e) => update("heartbeat.interval_seconds", parseInt(e.target.value))} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                    </div>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                      <div>
                        <label className="block text-sm text-secondary mb-1">Active From</label>
                        <input type="text" value={hb.active_hours_start || ""} onChange={(e) => update("heartbeat.active_hours_start", e.target.value)} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                      </div>
                      <div>
                        <label className="block text-sm text-secondary mb-1">Active To</label>
                        <input type="text" value={hb.active_hours_end || ""} onChange={(e) => update("heartbeat.active_hours_end", e.target.value)} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                      </div>
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Token Usage</h3>
                  <div className="text-sm text-secondary">Token usage tracking coming soon.</div>
                </section>
              </div>
            )}

            {activeTab === "channels" && (
              <div className="space-y-5">
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider">Configured Channels</h3>
                    <button
                      onClick={() => { setShowAddChannel(!showAddChannel); setAddChannelError(""); }}
                      className="px-3 py-1 rounded-md bg-primary-500 hover:bg-primary-600 text-white text-xs font-medium transition"
                    >
                      {showAddChannel ? "Cancel" : "+ Add"}
                    </button>
                  </div>

                  {showAddChannel && (
                    <div className="mb-4 p-4 rounded-lg bg-card space-y-3">
                      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-secondary mb-1">Name</label>
                          <input
                            type="text"
                            value={newChannel.name}
                            onChange={(e) => setNewChannel({ ...newChannel, name: e.target.value })}
                            placeholder="my-bot"
                            className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                          />
                        </div>
                        <div>
                          <label className="block text-xs text-secondary mb-1">Type</label>
                          <select
                            value={newChannel.channel_type}
                            onChange={(e) => setNewChannel({ ...newChannel, channel_type: e.target.value, credentials: {} })}
                            className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                          >
                            <option value="telegram">Telegram</option>
                            <option value="discord">Discord</option>
                            <option value="slack">Slack</option>
                            <option value="whatsapp">WhatsApp</option>
                            <option value="qq">QQ</option>
                            <option value="feishu">Feishu</option>
                            <option value="signal">Signal</option>
                            <option value="imessage">iMessage</option>
                            <option value="webchat">WebChat</option>
                            <option value="websocket">WebSocket</option>
                            <option value="web_terminal">Web Terminal</option>
                          </select>
                        </div>
                      </div>
                      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-secondary mb-1">Agent ID (optional)</label>
                          <input
                            type="text"
                            value={newChannel.agent_id}
                            onChange={(e) => setNewChannel({ ...newChannel, agent_id: e.target.value })}
                            placeholder="default"
                            className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                          />
                        </div>
                        <div className="flex items-center gap-2 pt-5">
                          <input
                            id="ch-enabled"
                            type="checkbox"
                            checked={newChannel.enabled}
                            onChange={(e) => setNewChannel({ ...newChannel, enabled: e.target.checked })}
                            className="rounded border-subtle text-primary-500 focus:ring-primary-500"
                          />
                          <label htmlFor="ch-enabled" className="text-sm text-secondary">Enabled</label>
                        </div>
                      </div>
                      {channelCredentialFields[newChannel.channel_type]?.map((field) => (
                        <div key={field.key}>
                          <label className="block text-xs text-secondary mb-1">{field.label}</label>
                          <input
                            type={field.type || "text"}
                            value={newChannel.credentials[field.key] || ""}
                            onChange={(e) => setNewChannel({
                              ...newChannel,
                              credentials: { ...newChannel.credentials, [field.key]: e.target.value },
                            })}
                            placeholder={field.label}
                            className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                          />
                        </div>
                      ))}
                      {addChannelError && (
                        <div className="text-xs text-red-600 dark:text-red-400">{addChannelError}</div>
                      )}
                      <div className="flex justify-end">
                        <button
                          onClick={handleAddChannel}
                          disabled={channelActionLoading === "add"}
                          className="px-4 py-1.5 rounded-md bg-primary-500 hover:bg-primary-600 disabled:opacity-50 text-white text-xs font-medium transition"
                        >
                          {channelActionLoading === "add" ? "Adding..." : "Add Channel"}
                        </button>
                      </div>
                    </div>
                  )}

                  {channels.length === 0 ? (
                    <div className="text-sm text-secondary">No channels configured.</div>
                  ) : (
                    <div className="space-y-2">
                      {channels.map((ch) => (
                        <div key={ch.name} className="flex items-center justify-between px-3 py-2 rounded-lg bg-card">
                          <div className="flex items-center gap-3">
                            <span className="text-sm text-primary font-medium">{ch.name}</span>
                            <span className="text-xs px-1.5 py-0.5 rounded bg-sidebar text-secondary uppercase">{ch.channel_type}</span>
                            {ch.agent_id && (
                              <span className="text-xs text-secondary font-mono">{ch.agent_id}</span>
                            )}
                            {ch.dm_policy && ch.dm_policy !== "open" && (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-400">{ch.dm_policy}</span>
                            )}
                            {ch.require_mention && (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-400">mention</span>
                            )}
                            {ch.has_credentials && (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400">auth</span>
                            )}
                          </div>
                          <div className="flex items-center gap-2">
                            <button
                              onClick={() => handleToggleChannel(ch.name, !ch.enabled)}
                              disabled={channelActionLoading === ch.name}
                              className={`text-xs px-2 py-0.5 rounded-full transition ${
                                ch.enabled
                                  ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400 hover:bg-primary-200 dark:hover:bg-primary-900/50"
                                  : "bg-sidebar text-secondary hover:bg-black/[0.06] dark:hover:bg-white/[0.08]"
                              }`}
                            >
                              {channelActionLoading === ch.name ? "..." : ch.enabled ? "Enabled" : "Disabled"}
                            </button>
                            <button
                              onClick={() => handleRemoveChannel(ch.name)}
                              disabled={channelActionLoading === ch.name}
                              className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-secondary/60 hover:text-red-600 dark:hover:text-red-400 transition"
                              title="Remove"
                            >
                              <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <line x1="18" y1="6" x2="6" y2="18" />
                                <line x1="6" y1="6" x2="18" y2="18" />
                              </svg>
                            </button>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "models" && (
              <div className="space-y-5">
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider">Available Models</h3>
                    <button
                      onClick={() => { setShowAddModel(!showAddModel); setAddModelError(""); }}
                      className="px-3 py-1 rounded-md bg-primary-500 hover:bg-primary-600 text-white text-xs font-medium transition"
                    >
                      {showAddModel ? "Cancel" : "+ Add"}
                    </button>
                  </div>

                  {showAddModel && (
                    <div className="mb-4 p-4 rounded-lg bg-card space-y-3">
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
                      <div>
                        <label className="block text-xs text-secondary mb-1">Base URL</label>
                        <input
                          type="text"
                          value={newModel.base_url}
                          onChange={(e) => setNewModel({ ...newModel, base_url: e.target.value })}
                          placeholder={modelPresets.find((p) => p.name === newModel.provider)?.base_url || "https://..."}
                          className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                        />
                      </div>
                      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-secondary mb-1">Name</label>
                          <input
                            type="text"
                            value={newModel.name}
                            onChange={(e) => setNewModel({ ...newModel, name: e.target.value })}
                            placeholder="smart"
                            className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                          />
                        </div>
                        {modelPresets.find((p) => p.name === newModel.provider)?.needs_api_key !== false && (
                          <div>
                            <label className="block text-xs text-secondary mb-1">API Key</label>
                            <input
                              type="password"
                              value={newModel.api_key}
                              onChange={(e) => setNewModel({ ...newModel, api_key: e.target.value })}
                              onBlur={() => {
                                if (newModel.api_key.trim() && remoteModels === null && !fetchingModels) {
                                  handleFetchModels();
                                }
                              }}
                              placeholder="sk-..."
                              className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                            />
                          </div>
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
                              <input
                                type="text"
                                value={newModel.model}
                                onChange={(e) => setNewModel({ ...newModel, model: e.target.value })}
                                placeholder="model-id"
                                className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
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
                  )}

                  {models.length === 0 ? (
                    <div className="text-sm text-secondary">No models available.</div>
                  ) : (
                    <div className="space-y-2">
                      {models.map((m) => (
                        <div key={m.id} className="flex items-center justify-between px-3 py-2 rounded-lg bg-card">
                          <div className="flex items-center gap-2">
                            <span className="text-sm text-primary font-medium">{m.name}</span>
                            <span className="text-xs text-secondary">{m.provider}</span>
                          </div>
                          <div className="flex items-center gap-2">
                            {config.model === m.id ? (
                              <span className="text-xs px-2 py-0.5 rounded-full bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400">Default</span>
                            ) : (
                              <button
                                onClick={() => handleSetDefaultModel(m.id)}
                                disabled={modelActionLoading === `default_${m.id}`}
                                className="text-xs px-2 py-0.5 rounded-full bg-sidebar text-secondary hover:bg-primary-100 dark:hover:bg-primary-900/30 hover:text-primary-700 dark:hover:text-primary-400 transition"
                              >
                                {modelActionLoading === `default_${m.id}` ? "..." : "Set Default"}
                              </button>
                            )}
                            <button
                              onClick={() => handleRemoveModel(m.id)}
                              disabled={modelActionLoading === m.id}
                              className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-secondary/60 hover:text-red-600 dark:hover:text-red-400 transition"
                              title="Remove"
                            >
                              <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <line x1="18" y1="6" x2="6" y2="18" />
                                <line x1="6" y1="6" x2="18" y2="18" />
                              </svg>
                            </button>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "agents" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Select Agent</h3>
                  {agentRegistry.length === 0 ? (
                    <div className="text-sm text-secondary">No agents in registry.</div>
                  ) : (
                    <select
                      value={selectedAgentId}
                      onChange={(e) => {
                        const id = e.target.value;
                        setSelectedAgentId(id);
                        loadAgentDetail(id);
                      }}
                      className="w-full rounded-lg border border-subtle bg-card px-3 py-2 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                    >
                      {agentRegistry.map((a) => (
                        <option key={a.id} value={a.id}>
                          {a.display_name || a.id}
                        </option>
                      ))}
                    </select>
                  )}
                </section>

                {agentDetailLoading && (
                  <div className="text-sm text-secondary flex items-center gap-2">
                    <div className="w-4 h-4 border-2 border-subtle border-t-primary-500 rounded-full animate-spin" />
                    Loading agent details...
                  </div>
                )}

                {selectedAgentDetail && !agentDetailLoading && (
                  <section className="space-y-4">
                    {/* Agent header */}
                    <div className="flex items-center gap-3">
                      <span className="text-sm font-medium text-primary">
                        {selectedAgentDetail.personality
                          ? String((selectedAgentDetail.personality as Record<string, unknown>).display_name ?? selectedAgentDetail.agent_id)
                          : selectedAgentDetail.agent_id}
                      </span>
                      <span className="text-xs text-secondary/70 font-mono">({selectedAgentDetail.agent_id})</span>
                    </div>

                    {/* Config */}
                    {selectedAgentDetail.config && (
                      <div>
                        <h4 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Configuration</h4>
                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                          <div className="px-3 py-2 rounded-lg bg-card">
                            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Temperature</div>
                            <div className="text-sm text-primary">{typeof (selectedAgentDetail.config as Record<string, unknown>).temperature === "number" ? ((selectedAgentDetail.config as Record<string, unknown>).temperature as number).toFixed(2) : "—"}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg bg-card">
                            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Max Tokens</div>
                            <div className="text-sm text-primary">{String((selectedAgentDetail.config as Record<string, unknown>).max_tokens ?? "—")}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg bg-card">
                            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Max Turns</div>
                            <div className="text-sm text-primary">{String((selectedAgentDetail.config as Record<string, unknown>).max_turns ?? "—")}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg bg-card">
                            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Max Concurrent Tools</div>
                            <div className="text-sm text-primary">{String((selectedAgentDetail.config as Record<string, unknown>).max_concurrent_tools ?? "—")}</div>
                          </div>
                        </div>
                        {"workspace_only" in (selectedAgentDetail.config as Record<string, unknown>) && (
                          <div className="mt-2 px-3 py-2 rounded-lg bg-card flex items-center justify-between">
                            <span className="text-sm text-secondary">Workspace Only</span>
                            <span className={`text-xs px-2 py-0.5 rounded-full ${(selectedAgentDetail.config as Record<string, unknown>).workspace_only ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400" : "bg-sidebar text-secondary"}`}>
                              {(selectedAgentDetail.config as Record<string, unknown>).workspace_only ? "Yes" : "No"}
                            </span>
                          </div>
                        )}
                      </div>
                    )}

                    {/* Personality */}
                    {selectedAgentDetail.personality && (
                      <div>
                        <h4 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Personality</h4>
                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                          <div className="px-3 py-2 rounded-lg bg-card">
                            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Display Name</div>
                            <div className="text-sm text-primary">{String((selectedAgentDetail.personality as Record<string, unknown>).display_name ?? "—")}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg bg-card">
                            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Valid</div>
                            <div className="text-sm text-primary">{(selectedAgentDetail.personality as Record<string, unknown>).is_valid ? "Yes" : "No"}</div>
                          </div>
                        </div>
                        <div className="mt-2 flex flex-wrap gap-1.5">
                          {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_heartbeat) && (
                            <span className="text-xs px-2 py-0.5 rounded-full bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400">Heartbeat</span>
                          )}
                          {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_soul) && (
                            <span className="text-xs px-2 py-0.5 rounded-full bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-400">Soul</span>
                          )}
                          {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_identity) && (
                            <span className="text-xs px-2 py-0.5 rounded-full bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-400">Identity</span>
                          )}
                          {Boolean((selectedAgentDetail.personality as Record<string, unknown>).has_memory) && (
                            <span className="text-xs px-2 py-0.5 rounded-full bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400">Memory</span>
                          )}
                        </div>
                      </div>
                    )}
                  </section>
                )}

                <section>
                  {(() => {
                    const hasAgentCfg = selectedAgentDetail?.config != null;
                    const ac = (selectedAgentDetail?.config as Record<string, unknown> | null) ?? (da as Record<string, unknown>);
                    return (
                      <>
                        <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">
                          {hasAgentCfg ? `${selectedAgentDetail!.agent_id} Parameters` : "Global Default Parameters"}
                        </h3>
                        {hasAgentCfg && (
                          <div className="text-[11px] text-secondary/70 mb-2">Editing individual agent parameters is not yet supported. Changes here affect the global default.</div>
                        )}
                        <div className="space-y-3">
                          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                            <div>
                              <label className="block text-sm text-secondary mb-1">Temperature</label>
                              <div className="flex items-center gap-2">
                                <input type="range" min="0" max="2" step="0.1" value={(ac.temperature as number | undefined) ?? 0.7} onChange={(e) => update("default_agent.temperature", parseFloat(e.target.value))} className="flex-1 h-1.5 bg-secondary/20 dark:bg-secondary/20 rounded-lg appearance-none cursor-pointer accent-primary-500" />
                                <span className="text-sm text-secondary w-10 text-right tabular-nums">{((ac.temperature as number | undefined) ?? 0.7).toFixed(2)}</span>
                              </div>
                            </div>
                            <div>
                              <label className="block text-sm text-secondary mb-1">Max Tokens</label>
                              <input type="number" value={(ac.max_tokens as number | undefined) ?? 2048} onChange={(e) => update("default_agent.max_tokens", parseInt(e.target.value))} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                            </div>
                          </div>
                          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                            <div>
                              <label className="block text-sm text-secondary mb-1">Max Turns</label>
                              <input type="number" value={(ac.max_turns as number | undefined) ?? ""} placeholder="Unlimited" onChange={(e) => update("default_agent.max_turns", e.target.value ? parseInt(e.target.value) : null)} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                            </div>
                            <div>
                              <label className="block text-sm text-secondary mb-1">Max Concurrent Tools</label>
                              <input type="number" value={(ac.max_concurrent_tools as number | undefined) ?? 5} onChange={(e) => update("default_agent.max_concurrent_tools", parseInt(e.target.value))} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                            </div>
                          </div>
                          <div>
                            <label className="block text-sm text-secondary mb-1">System Prompt</label>
                            <textarea value={(ac.system_prompt as string | undefined) || ""} onChange={(e) => update("default_agent.system_prompt", e.target.value)} className="w-full h-[60vh] rounded-lg border border-subtle bg-card px-3 py-2 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20 resize-none font-mono" />
                          </div>
                        </div>
                      </>
                    );
                  })()}
                </section>
              </div>
            )}

            {activeTab === "mcp" && (
              <div className="space-y-5">
                {mcpPresets.length > 0 && (
                  <section>
                    <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-3">Presets</h3>
                    <div className="grid grid-cols-1 sm:grid-cols-2 sm:grid-cols-3 md:grid-cols-2 sm:grid-cols-4 gap-2">
                      {mcpPresets.map((p) => {
                        const loading = mcpActionLoading === p.name;
                        return (
                          <div
                            key={p.name}
                            className={`flex flex-col items-start gap-1 px-3 py-2.5 rounded-lg border text-left text-xs transition ${
                              p.enabled
                                ? "border-primary-400 bg-primary-100 dark:bg-primary-900/30"
                                : "border-subtle"
                            } ${loading ? "opacity-50" : ""}`}
                          >
                            <div className="flex items-center gap-1.5 w-full">
                              {p.logo_url && (
                                <img src={p.logo_url} alt="" className="w-4 h-4 object-contain shrink-0" />
                              )}
                              <span className="font-medium text-sm flex-1">{p.display_name}</span>
                              <button
                                type="button"
                                disabled={loading}
                                onClick={() => (p.enabled ? handleDisablePreset(p.name) : handleEnablePreset(p))}
                                className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                                  loading
                                    ? "opacity-50 cursor-not-allowed"
                                    : "cursor-pointer"
                                } ${
                                  p.enabled
                                    ? "bg-primary-500"
                                    : "bg-gray-300 dark:bg-gray-600"
                                }`}
                                role="switch"
                                aria-checked={p.enabled}
                              >
                                <span
                                  className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform ${
                                    p.enabled ? "translate-x-[18px]" : "translate-x-[3px]"
                                  }`}
                                />
                              </button>
                            </div>
                            <span className="text-[11px] leading-tight opacity-70 line-clamp-2">{p.description}</span>
                            {p.enabled && p.env?.length ? (
                              <button
                                type="button"
                                disabled={loading}
                                onClick={() => setEnvModal({ preset: p, values: {} })}
                                className="text-[10px] px-1.5 py-0.5 rounded bg-sidebar text-secondary hover:text-primary transition"
                              >
                                Configure tokens
                              </button>
                            ) : null}
                          </div>
                        );
                      })}
                    </div>
                  </section>
                )}
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider">MCP Servers</h3>
                    <button onClick={() => setShowAddMcp(true)} className="text-xs px-2 py-1 rounded bg-primary-500 text-white hover:bg-primary-600 transition">+ Add</button>
                  </div>
                  {showAddMcp && (
                    <div className="mb-4 p-4 rounded-lg bg-card space-y-3">
                      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-secondary mb-1">Server ID</label>
                          <input type="text" value={newMcp.id} onChange={(e) => setNewMcp({ ...newMcp, id: e.target.value })} placeholder="filesystem" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                        </div>
                        <div>
                          <label className="block text-xs text-secondary mb-1">Transport</label>
                          <select value={newMcp.transport} onChange={(e) => setNewMcp({ ...newMcp, transport: e.target.value })} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20">
                            <option value="stdio">stdio</option>
                            <option value="sse">sse</option>
                            <option value="streamable_http">streamable_http</option>
                          </select>
                        </div>
                      </div>
                      {newMcp.transport === "stdio" ? (
                        <>
                          <div>
                            <label className="block text-xs text-secondary mb-1">Command</label>
                            <input type="text" value={newMcp.command} onChange={(e) => setNewMcp({ ...newMcp, command: e.target.value })} placeholder="npx -y @modelcontextprotocol/server-filesystem" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                          </div>
                          <div>
                            <label className="block text-xs text-secondary mb-1">Args (comma-separated)</label>
                            <input type="text" value={newMcp.args} onChange={(e) => setNewMcp({ ...newMcp, args: e.target.value })} placeholder="/home/user/docs" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                          </div>
                        </>
                      ) : (
                        <>
                          <div>
                            <label className="block text-xs text-secondary mb-1">URL</label>
                            <input type="text" value={newMcp.url} onChange={(e) => setNewMcp({ ...newMcp, url: e.target.value })} placeholder="http://localhost:3000/sse" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                          </div>
                          <div>
                            <label className="block text-xs text-secondary mb-1">Auth Type</label>
                            <select value={newMcp.auth_type} onChange={(e) => setNewMcp({ ...newMcp, auth_type: e.target.value })} className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20">
                              <option value="">none</option>
                              <option value="oauth2">oauth2</option>
                            </select>
                          </div>
                          {newMcp.auth_type === "oauth2" && (
                            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                              <div>
                                <label className="block text-xs text-secondary mb-1">Client ID</label>
                                <input type="text" value={newMcp.client_id} onChange={(e) => setNewMcp({ ...newMcp, client_id: e.target.value })} placeholder="your-client-id" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                              </div>
                              <div>
                                <label className="block text-xs text-secondary mb-1">Scopes</label>
                                <input type="text" value={newMcp.scopes} onChange={(e) => setNewMcp({ ...newMcp, scopes: e.target.value })} placeholder="read write" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                              </div>
                              <div>
                                <label className="block text-xs text-secondary mb-1">Auth URL</label>
                                <input type="text" value={newMcp.auth_url} onChange={(e) => setNewMcp({ ...newMcp, auth_url: e.target.value })} placeholder="http://localhost:9999/auth (optional, discoverable)" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                              </div>
                              <div>
                                <label className="block text-xs text-secondary mb-1">Token URL</label>
                                <input type="text" value={newMcp.token_url} onChange={(e) => setNewMcp({ ...newMcp, token_url: e.target.value })} placeholder="http://localhost:9999/token (optional, discoverable)" className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20" />
                              </div>
                            </div>
                          )}
                        </>
                      )}
                      <div className="flex items-center gap-2">
                        <input id="mcp-auto" type="checkbox" checked={newMcp.auto_connect} onChange={(e) => setNewMcp({ ...newMcp, auto_connect: e.target.checked })} className="rounded border-subtle text-primary-500 focus:ring-primary-500" />
                        <label htmlFor="mcp-auto" className="text-sm text-secondary">Auto-connect</label>
                      </div>
                      {addMcpError && <div className="text-xs text-red-600 dark:text-red-400">{addMcpError}</div>}
                      <div className="flex justify-end">
                        <button onClick={handleAddMcp} disabled={mcpActionLoading === "add"} className="px-4 py-1.5 rounded-md bg-primary-500 hover:bg-primary-600 disabled:opacity-50 text-white text-xs font-medium transition">
                          {mcpActionLoading === "add" ? "Adding..." : "Add Server"}
                        </button>
                      </div>
                    </div>
                  )}
                  {mcpServers.length === 0 ? (
                    <div className="text-sm text-secondary">No MCP servers configured.</div>
                  ) : (
                    <div className="space-y-2">
                      {mcpServers.map((srv) => (
                        <div key={srv.id} className="flex items-center justify-between px-3 py-2 rounded-lg bg-card">
                          <div className="flex items-center gap-3">
                            <span className="text-sm text-primary font-medium">
                              {srv.id.charAt(0).toUpperCase() + srv.id.slice(1)}
                            </span>
                            <span className="text-xs px-1.5 py-0.5 rounded bg-sidebar text-secondary uppercase">{srv.transport}</span>
                            {srv.connected ? (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400">connected</span>
                            ) : (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-sidebar text-secondary">disconnected</span>
                            )}
                          </div>
                          <div className="flex items-center gap-2">
                            {srv.connected ? (
                              <button onClick={() => handleDisconnectMcp(srv.id)} disabled={mcpActionLoading === srv.id} className="text-xs px-2 py-0.5 rounded bg-sidebar text-secondary hover:bg-black/[0.06] dark:hover:bg-white/[0.08] transition">
                                {mcpActionLoading === srv.id ? "..." : "Disconnect"}
                              </button>
                            ) : (
                              <button onClick={() => handleConnectMcp(srv.id)} disabled={mcpActionLoading === srv.id} className="text-xs px-2 py-0.5 rounded bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400 hover:bg-primary-200 dark:hover:bg-primary-900/50 transition">
                                {mcpActionLoading === srv.id ? "..." : "Connect"}
                              </button>
                            )}
                            <button onClick={() => handleRemoveMcp(srv.id)} disabled={mcpActionLoading === srv.id} className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-secondary/60 hover:text-red-600 dark:hover:text-red-400 transition" title="Remove">
                              <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></svg>
                            </button>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "jobs" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Jobs ({crons.length})</h3>
                  {crons.length === 0 ? (
                    <div className="text-sm text-secondary">No cron jobs configured.</div>
                  ) : (
                    <div className="space-y-2">
                      {crons.map((job, i) => {
                        const j = job as Record<string, unknown>;
                        const target = j.target as Record<string, unknown> | undefined;
                        const targetType = target?.type as string | undefined;
                        const jobState = j.state as Record<string, unknown> | undefined;
                        const nextRun = jobState?.next_run_at as string | undefined;
                        const lastRun = jobState?.last_run_at as string | undefined;
                        const agentId = target?.agent_id as string | undefined;
                        const command = target?.command as string | undefined;
                        const prompt = target?.prompt as string | undefined;
                        return (
                          <div key={i} className="px-3 py-2 rounded-lg bg-card">
                            <div className="flex items-center justify-between">
                              <span className="text-sm text-primary font-medium">{(j.name as string) || "Unnamed"}</span>
                              <span className={`text-xs px-2 py-0.5 rounded-full ${j.enabled ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400" : "bg-sidebar text-secondary"}`}>
                                {j.enabled ? "Enabled" : "Disabled"}
                              </span>
                            </div>
                            <div className="mt-1.5 space-y-1">
                              {(() => {
                                const sched = j.schedule as Record<string, unknown> | string | undefined;
                                const expr = typeof sched === "string" ? sched : (sched as Record<string, unknown> | undefined)?.expression as string | undefined;
                                if (!expr) return null;
                                return (
                                  <div className="flex items-center gap-2">
                                    <span className="text-[10px] uppercase tracking-wider text-secondary/70 w-16 shrink-0">Schedule</span>
                                    <span className="text-xs text-secondary font-mono">{expr}</span>
                                  </div>
                                );
                              })()}
                              {nextRun && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-secondary/70 w-16 shrink-0">Next Run</span>
                                  <span className="text-xs text-secondary">{new Date(nextRun).toLocaleString()}</span>
                                </div>
                              )}
                              {lastRun && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-secondary/70 w-16 shrink-0">Last Run</span>
                                  <span className="text-xs text-secondary">{new Date(lastRun).toLocaleString()}</span>
                                </div>
                              )}
                              <div className="flex items-center gap-2">
                                <span className="text-[10px] uppercase tracking-wider text-secondary/70 w-16 shrink-0">Target</span>
                                {targetType === "shell" ? (
                                  <span className="text-xs px-1.5 py-0.5 rounded bg-sidebar text-secondary">Shell</span>
                                ) : targetType === "agent" ? (
                                  <span className="text-xs px-1.5 py-0.5 rounded bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-400">Agent</span>
                                ) : (
                                  <span className="text-xs text-secondary">{targetType || "Unknown"}</span>
                                )}
                              </div>
                              {targetType === "agent" && agentId && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-secondary/70 w-16 shrink-0">Agent</span>
                                  <span className="text-xs text-secondary font-mono">{agentId}</span>
                                </div>
                              )}
                              {targetType === "shell" && command && (
                                <div className="flex items-start gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-secondary/70 w-16 shrink-0">Command</span>
                                  <span className="text-xs text-secondary font-mono break-all">{command}</span>
                                </div>
                              )}
                              {targetType === "agent" && prompt && (
                                <div className="flex items-start gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-secondary/70 w-16 shrink-0">Prompt</span>
                                  <span className="text-xs text-secondary line-clamp-2">{prompt}</span>
                                </div>
                              )}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </section>
              </div>
            )}


            {activeTab === "skills" && (
              <div className="space-y-5">
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider">Skills ({skills.length})</h3>
                    <button
                      onClick={() => { setShowAddSkill(!showAddSkill); setAddSkillError(""); }}
                      className="px-3 py-1 rounded-md bg-primary-500 hover:bg-primary-600 text-white text-xs font-medium transition"
                    >
                      {showAddSkill ? "Cancel" : "+ Install"}
                    </button>
                  </div>

                  {showAddSkill && (
                    <div className="mb-4 p-4 rounded-lg bg-card space-y-3">
                      <div>
                        <label className="block text-xs text-secondary mb-1">Skill Name</label>
                        <input
                          type="text"
                          value={newSkillName}
                          onChange={(e) => setNewSkillName(e.target.value)}
                          placeholder="my-skill"
                          className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                        />
                      </div>
                      <div>
                        <label className="block text-xs text-secondary mb-1">ZIP File</label>
                        <input
                          type="file"
                          accept=".zip"
                          onChange={(e) => setNewSkillZip(e.target.files?.[0] || null)}
                          className="w-full text-sm text-secondary file:mr-3 file:py-1.5 file:px-3 file:rounded-md file:border-0 file:text-xs file:font-medium file:bg-primary-50 file:text-primary-700 dark:file:bg-primary-900/20 dark:file:text-primary-400 hover:file:bg-primary-100"
                        />
                        <p className="text-[10px] text-secondary/70 mt-1">ZIP must contain a SKILL.md file at the root.</p>
                      </div>
                      {addSkillError && (
                        <div className="text-xs text-red-600 dark:text-red-400">{addSkillError}</div>
                      )}
                      <div className="flex justify-end">
                        <button
                          onClick={handleAddSkill}
                          disabled={skillActionLoading === "add"}
                          className="px-4 py-1.5 rounded-md bg-primary-500 hover:bg-primary-600 disabled:opacity-50 text-white text-xs font-medium transition"
                        >
                          {skillActionLoading === "add" ? "Installing..." : "Install Skill"}
                        </button>
                      </div>
                    </div>
                  )}

                  {skills.length === 0 ? (
                    <div className="text-sm text-secondary">No skills loaded.</div>
                  ) : (
                    <div className="space-y-2">
                      {skills.map((s, i) => {
                        const sk = s as Record<string, unknown>;
                        const triggers = (sk.triggers as Array<Record<string, unknown>>) || [];
                        const deps = sk.depends_on as Record<string, string> | undefined;
                        const provides = (sk.provides as string[]) || [];
                        const chain = (sk.chain as string[]) || [];
                        return (
                          <div key={i} className="px-3 py-2 rounded-lg bg-card">
                            <div className="flex items-center justify-between">
                              <div className="flex items-center gap-2">
                                <span className="text-sm text-primary font-medium">{String(sk.name || "Unnamed")}</span>
                                <span className="text-xs text-secondary">{String(sk.version || "")}</span>
                              </div>
                              {Boolean(sk.author) && (
                                <span className="text-xs text-secondary/70">by {String(sk.author)}</span>
                              )}
                            </div>
                            {Boolean(sk.description) && (
                              <div className="text-xs text-secondary mt-1">{String(sk.description)}</div>
                            )}
                            <div className="mt-1.5 flex flex-wrap gap-1.5">
                              {triggers.map((t, ti) => (
                                <span key={ti} className="text-[10px] px-1.5 py-0.5 rounded bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-400">
                                  {String(t.type || "")}: {String(t.pattern || "")}
                                </span>
                              ))}
                            </div>
                            {provides.length > 0 && (
                              <div className="mt-1 flex flex-wrap gap-1">
                                {provides.map((p, pi) => (
                                  <span key={pi} className="text-[10px] px-1.5 py-0.5 rounded bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-400">
                                    {p}
                                  </span>
                                ))}
                              </div>
                            )}
                            {deps && Object.keys(deps).length > 0 && (
                              <div className="mt-1 text-[10px] text-secondary/70">
                                deps: {Object.entries(deps).map(([k, v]) => `${k}@${v}`).join(", ")}
                              </div>
                            )}
                            {chain.length > 0 && (
                              <div className="mt-1 text-[10px] text-secondary/70">
                                chain: {chain.join(" → ")}
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}
                </section>
              </div>
            )}

            {activeTab === "tools" && (
              <div className="space-y-5">
                {/* Default Provider */}
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Default Search Provider</h3>
                  <select
                    value={config.search?.provider ?? "duckduckgo"}
                    onChange={(e) => update("search.provider", e.target.value)}
                    className="w-full text-sm border border-subtle rounded-lg px-3 py-2 bg-card text-primary"
                  >
                    {SEARCH_PROVIDERS.map((p) => (
                      <option key={p.id} value={p.id}>{p.label}</option>
                    ))}
                  </select>
                </section>

                {/* Fallback Provider Order */}
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Fallback Order</h3>
                  <div className="flex flex-wrap gap-2 mb-2">
                    {(config.search?.providers ?? []).map((prov) => (
                      <span key={prov} className="inline-flex items-center gap-1 px-2.5 py-1 rounded-full text-xs bg-sidebar text-secondary">
                        {prov}
                        <button
                          onClick={() => {
                            const updated = (config.search?.providers ?? []).filter((p) => p !== prov);
                            update("search.providers", updated);
                          }}
                          className="hover:text-red-500 transition"
                        >
                          <X size={12} />
                        </button>
                      </span>
                    ))}
                  </div>
                  <div className="flex gap-2">
                    <select
                      value=""
                      onChange={(e) => {
                        const val = e.target.value;
                        if (val) {
                          const current = config.search?.providers ?? [];
                          if (!current.includes(val)) {
                            update("search.providers", [...current, val]);
                          }
                        }
                      }}
                      className="text-sm border border-subtle rounded-lg px-3 py-2 bg-card text-primary"
                    >
                      <option value="">+ Add Provider</option>
                      {SEARCH_PROVIDERS.filter((p) => !(config.search?.providers ?? []).includes(p.id)).map((p) => (
                        <option key={p.id} value={p.id}>{p.label}</option>
                      ))}
                    </select>
                  </div>
                </section>

                {/* API Keys */}
                <section>
                  <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Provider API Keys</h3>
                  <div className="space-y-3">
                    {SEARCH_PROVIDERS.filter((p) => p.needsKey).map((p) => (
                      <div key={p.id} className="flex items-center gap-3">
                        <label className="w-28 text-sm text-secondary shrink-0">{p.label}</label>
                        <input
                          type="password"
                          placeholder={config.search?.keys?.[p.id] === "true" ? "••••••••" : ""}
                          value=""
                          onChange={(e) => update(`search.keys.${p.id}`, e.target.value)}
                          onFocus={(e) => (e.target.value = "")}
                          className="flex-1 text-sm border border-subtle rounded-lg px-3 py-2 bg-card text-primary placeholder-gray-400"
                        />
                      </div>
                    ))}
                    {/* Google CX special case */}
                    <div className="flex items-center gap-3">
                      <label className="w-28 text-sm text-secondary shrink-0">Google CX</label>
                      <input
                        type="password"
                        placeholder={config.search?.keys?.google_cx === "true" ? "••••••••" : ""}
                        value=""
                        onChange={(e) => update("search.keys.google_cx", e.target.value)}
                        onFocus={(e) => (e.target.value = "")}
                        className="flex-1 text-sm border border-subtle rounded-lg px-3 py-2 bg-card text-primary placeholder-gray-400"
                      />
                    </div>
                  </div>
                </section>
              </div>
            )}

            {activeTab === "devices" && (
              <div className="space-y-5">
                {!transport.isTauri() || (deviceCaps === null && !deviceCapsLoading) ? (
                  <section>
                    <div className="rounded-lg bg-card border border-subtle px-4 py-6 text-center text-sm text-secondary">
                      Device capabilities (camera, location, notifications, wireless debugging)
                      are available in the Syscity mobile app.
                    </div>
                  </section>
                ) : deviceCapsLoading ? (
                  <div className="text-sm text-secondary py-6 text-center">Loading...</div>
                ) : (
                  <>
                    <section>
                      <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Capabilities</h3>
                      <div className="space-y-2">
                        {(deviceCaps || []).map((cap) => (
                          <div key={cap.id} className="flex items-center justify-between px-3 py-2 rounded-lg bg-card">
                            <div className="flex items-center gap-2">
                              <span className="text-secondary">{deviceCapIcon[cap.id]}</span>
                              <span className="text-sm text-primary">{cap.label}</span>
                              <span className="text-[10px] text-secondary/70 font-mono">{cap.id}</span>
                            </div>
                            <div className="flex items-center gap-2">
                              <span
                                className={`text-xs px-2 py-0.5 rounded-full ${
                                  cap.granted
                                    ? "bg-green-500/10 text-green-600 dark:text-green-400"
                                    : "bg-amber-500/10 text-amber-600 dark:text-amber-400"
                                }`}
                              >
                                {cap.granted ? "Granted" : "Not granted"}
                              </span>
                              {needsDevicePermission(cap.id) && (
                                <button
                                  onClick={() => requestDevicePermission(cap.id)}
                                  disabled={permRequesting === cap.id}
                                  className="px-2.5 py-1 text-xs rounded-md bg-primary-600 text-white hover:opacity-90 transition-opacity disabled:opacity-50"
                                >
                                  {permRequesting === cap.id ? "Requesting..." : "Request"}
                                </button>
                              )}
                            </div>
                          </div>
                        ))}
                      </div>
                    </section>

                    {transport.isTauri() && isIOSDevice && (
                      <section>
                        <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Shortcuts</h3>
                        <div className="rounded-lg bg-card border border-subtle p-3 space-y-3">
                          <p className="text-xs text-secondary">
                            Run an iOS Shortcut from Syscity. The shortcut opens in the Shortcuts app;
                            if its final step is "Save Syscity Output", the output is returned here for
                            the agent to read. "Ask Syscity" inboxes prompts from Siri / automations.
                          </p>
                          <div className="grid grid-cols-2 gap-2">
                            <div>
                              <label className="block text-xs text-secondary mb-1">Shortcut name</label>
                              <input
                                placeholder="e.g. Order Coffee"
                                value={shortcutName}
                                onChange={(e) => setShortcutName(e.target.value)}
                                className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                              />
                            </div>
                            <div>
                              <label className="block text-xs text-secondary mb-1">Input (optional)</label>
                              <input
                                placeholder="Text to pass to the shortcut"
                                value={shortcutInput}
                                onChange={(e) => setShortcutInput(e.target.value)}
                                className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                              />
                            </div>
                          </div>
                          <div className="flex items-center gap-2">
                            <button
                              onClick={runShortcut}
                              disabled={shortcutRunning}
                              className="px-3 py-1.5 text-xs font-medium rounded-lg bg-primary-600 text-white hover:opacity-90 transition-opacity disabled:opacity-50"
                            >
                              {shortcutRunning ? "Running..." : "Run"}
                            </button>
                            <button
                              onClick={refreshShortcutResults}
                              className="px-3 py-1.5 text-xs font-medium rounded-lg bg-card border border-subtle text-primary hover:bg-accent/50 transition-colors"
                            >
                              Fetch outputs
                            </button>
                            <button
                              onClick={refreshShortcutInbox}
                              className="px-3 py-1.5 text-xs font-medium rounded-lg bg-card border border-subtle text-primary hover:bg-accent/50 transition-colors"
                            >
                              Fetch inbox
                            </button>
                          </div>
                          {shortcutMsg && (
                            <div className="text-xs text-secondary break-words">{shortcutMsg}</div>
                          )}
                          <div className="grid grid-cols-2 gap-2">
                            <div>
                              <div className="text-[10px] text-secondary/70 uppercase tracking-wider mb-1">Outputs</div>
                              {shortcutResults.length === 0 ? (
                                <div className="text-xs text-secondary">None pending</div>
                              ) : (
                                shortcutResults.map((r, i) => (
                                  <div key={i} className="text-xs text-primary font-mono break-all bg-accent/30 rounded px-2 py-1 mb-1">
                                    {r.output || "(no output)"}
                                    {r.at_ms ? <div className="text-[10px] text-secondary/70">{new Date(r.at_ms).toLocaleTimeString()}</div> : null}
                                  </div>
                                ))
                              )}
                            </div>
                            <div>
                              <div className="text-[10px] text-secondary/70 uppercase tracking-wider mb-1">Inbox</div>
                              {shortcutInbox.length === 0 ? (
                                <div className="text-xs text-secondary">None pending</div>
                              ) : (
                                shortcutInbox.map((p, i) => (
                                  <div key={i} className="text-xs text-primary font-mono break-all bg-accent/30 rounded px-2 py-1 mb-1">
                                    {p.prompt || "(no prompt)"}
                                    {p.at_ms ? <div className="text-[10px] text-secondary/70">{new Date(p.at_ms).toLocaleTimeString()}</div> : null}
                                  </div>
                                ))
                              )}
                            </div>
                          </div>
                        </div>
                      </section>
                    )}

                    <section>
                      <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider mb-2">Wireless debugging</h3>
                      <div className="rounded-lg bg-card border border-subtle p-3 space-y-3">
                        <p className="text-xs text-secondary">
                          Pair this phone with its own wireless-debugging adb server for on-device
                          automation (screenshots, input, UI tree). On the phone: enable Developer
                          options → Wireless debugging, then use "Pair device with pairing code".
                        </p>
                        <div className="grid grid-cols-2 gap-2">
                          <div>
                            <label className="block text-xs text-secondary mb-1">Pairing port</label>
                            <input
                              inputMode="numeric"
                              placeholder="e.g. 45678"
                              value={adbPort}
                              onChange={(e) => setAdbPort(e.target.value)}
                              className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                            />
                          </div>
                          <div>
                            <label className="block text-xs text-secondary mb-1">Connect port (optional)</label>
                            <input
                              inputMode="numeric"
                              placeholder="e.g. 45679"
                              value={adbConnectPort}
                              onChange={(e) => setAdbConnectPort(e.target.value)}
                              className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                            />
                          </div>
                        </div>
                        <div>
                          <label className="block text-xs text-secondary mb-1">Pairing code</label>
                          <input
                            inputMode="numeric"
                            placeholder="6-digit code"
                            value={adbCode}
                            onChange={(e) => setAdbCode(e.target.value)}
                            className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                          />
                        </div>
                        <div className="flex items-center gap-2">
                          <button
                            onClick={pairAdb}
                            disabled={adbPairing}
                            className="px-3 py-1.5 text-xs font-medium rounded-lg bg-primary-600 text-white hover:opacity-90 transition-opacity disabled:opacity-50"
                          >
                            {adbPairing ? "Pairing..." : "Pair"}
                          </button>
                          <span className="text-xs text-secondary">Pairing is per-boot.</span>
                        </div>
                        {adbError && (
                          <div className="text-xs text-red-600 dark:text-red-400 break-words">{adbError}</div>
                        )}
                        <div className="border-t border-subtle pt-2">
                          <div className="flex items-center justify-between">
                            <span className="text-xs text-secondary">Status</span>
                            <button onClick={refreshAdbStatus} className="text-xs text-primary-600 hover:underline">
                              Refresh
                            </button>
                          </div>
                          {adbStatus === null ? (
                            <div className="text-xs text-secondary mt-1">Unknown</div>
                          ) : adbStatus.paired ? (
                            <div className="text-xs text-green-600 dark:text-green-400 mt-1">Paired</div>
                          ) : (
                            <div className="text-xs text-secondary mt-1">Not paired</div>
                          )}
                          {adbStatus && adbStatus.devices.length > 0 && (
                            <div className="mt-1 font-mono text-[11px] text-secondary">
                              {adbStatus.devices.map((d) => `${d.serial} (${d.state})`).join(", ")}
                            </div>
                          )}
                        </div>
                      </div>
                    </section>
                  </>
                )}
              </div>
            )}

            {activeTab === "logs" && (
              <div className="flex flex-col h-full">
                <section className="flex flex-col flex-1">
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider">Logs</h3>
                    <span className={`text-[10px] px-2 py-0.5 rounded-full ${logsSubscribed ? 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-400' : 'bg-sidebar text-secondary/70'}`}>
                      {logsSubscribed ? "Live" : "Disconnected"}
                    </span>
                  </div>
                  <div
                    ref={logListRef}
                    className="bg-sidebar rounded-lg h-[90vh] overflow-y-auto font-mono text-[11px] leading-4 p-3"
                  >
                    {logLines.length === 0 && (
                      <div className="text-secondary/50 text-center py-20">
                        {logsSubscribed ? "Waiting for logs..." : "Click the Logs tab to connect"}
                      </div>
                    )}
                    {logLines.map((line, i) => (
                      <div key={i} className="text-secondary whitespace-pre-wrap break-all py-0.5 border-b border-subtle last:border-0">
                        {line}
                      </div>
                    ))}
                  </div>
                  <div className="flex gap-2 mt-2">
                    <button
                      onClick={() => setLogLines([])}
                      className="px-3 py-1.5 text-xs bg-sidebar hover:bg-black/5 dark:hover:bg-white/5 rounded-md text-secondary transition-colors"
                    >
                      Clear
                    </button>
                    <button
                      onClick={() => {
                        const blob = new Blob([logLines.join("\n")], { type: "text/plain" });
                        const url = URL.createObjectURL(blob);
                        const a = document.createElement("a");
                        a.href = url;
                        a.download = `syscity-logs-${new Date().toISOString().slice(0, 19)}.txt`;
                        a.click();
                        URL.revokeObjectURL(url);
                      }}
                      disabled={logLines.length === 0}
                      className="px-3 py-1.5 text-xs bg-sidebar hover:bg-black/5 dark:hover:bg-white/5 rounded-md text-secondary transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                      Download
                    </button>
                  </div>
                </section>
              </div>
            )}
          </div>
        </div>
      )}

      {/* OAuth authorization modal */}
      {authModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="bg-card rounded-xl p-6 max-w-md w-full mx-4 shadow-xl">
            <h3 className="text-sm font-semibold mb-2">Authorize MCP Server</h3>
            <p className="text-xs text-secondary mb-4">
              This server needs you to authorize with your account. Click the button below to
              open your browser and complete the authorization.
            </p>
            <div className="flex gap-2">
              <button
                onClick={() => window.open(authModal.authUrl, "_blank")}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-primary-600 text-white hover:opacity-90 transition-opacity"
              >
                Authorize in Browser
              </button>
              <button
                onClick={handleCancelAuth}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-sidebar text-secondary hover:text-primary transition-colors"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Env token modal — collects required tokens for a preset, validates on save */}
      {envModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="bg-card rounded-xl p-6 max-w-md w-full mx-4 shadow-xl">
            <h3 className="text-sm font-semibold mb-1">Configure {envModal.preset.display_name}</h3>
            <p className="text-xs text-secondary mb-4">
              Enter the tokens this MCP server needs. They are stored securely on this machine
              and verified by connecting before enabling.
            </p>
            <div className="space-y-3">
              {envModal.preset.env.map((v) => (
                <div key={v.name}>
                  <label className="block text-xs text-secondary mb-1">
                    {v.name}
                    {v.required && <span className="text-red-500"> *</span>}
                  </label>
                  <input
                    type="password"
                    placeholder="••••••••"
                    value={envModal.values[v.name] ?? ""}
                    onChange={(e) =>
                      setEnvModal((m) =>
                        m ? { ...m, values: { ...m.values, [v.name]: e.target.value } } : m
                      )
                    }
                    className="w-full rounded-lg border border-subtle bg-card px-3 py-1.5 text-sm text-primary placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-primary-500/20"
                  />
                  {v.description && (
                    <p className="text-[10px] text-secondary/70 mt-0.5">{v.description}</p>
                  )}
                </div>
              ))}
            </div>
            {envModal.error && (
              <div className="mt-3 text-xs text-red-600 dark:text-red-400 break-words">{envModal.error}</div>
            )}
            <div className="flex gap-2 mt-5">
              <button
                onClick={submitEnv}
                disabled={envModal.saving}
                className="flex-1 px-4 py-2 text-xs font-medium rounded-lg bg-primary-600 text-white hover:opacity-90 transition-opacity disabled:opacity-50"
              >
                {envModal.saving ? "Validating..." : "Save & Enable"}
              </button>
              <button
                onClick={() => setEnvModal(null)}
                disabled={envModal.saving}
                className="px-4 py-2 text-xs font-medium rounded-lg bg-sidebar text-secondary hover:text-primary transition-colors"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Toast notification */}
      {toast && (
        <div
          className={`fixed bottom-6 left-1/2 -translate-x-1/2 z-50 px-4 py-2 rounded-lg text-sm shadow-lg transition-all ${
            toast.type === "success"
              ? "bg-green-600 text-white"
              : "bg-red-600 text-white"
          }`}
        >
          {toast.message}
        </div>
      )}
    </div>
  );
}
