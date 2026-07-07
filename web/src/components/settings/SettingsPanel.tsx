import { useState, useEffect, useRef } from "react";
import { X } from "lucide-react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";

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
  const [agentRegistry, setAgentRegistry] = useState<Array<{ id: string; display_name: string; is_valid: boolean; has_heartbeat: boolean }>>([]);
  const [sessions, setSessions] = useState<Array<{ id: string; label?: string }>>([]);
  const [crons, setCrons] = useState<Array<Record<string, unknown>>>([]);
  const [skills, setSkills] = useState<Array<Record<string, unknown>>>([]);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState("general");
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
  const [modelPresets, setModelPresets] = useState<Array<{ name: string; display_name: string; base_url?: string; models: string[] }>>([]);
  const [showAddSkill, setShowAddSkill] = useState(false);
  const [addSkillError, setAddSkillError] = useState("");
  const [newSkillName, setNewSkillName] = useState("");
  const [newSkillZip, setNewSkillZip] = useState<File | null>(null);
  const [skillActionLoading, setSkillActionLoading] = useState<string>("");
  const [logLines, setLogLines] = useState<string[]>([]);
  const [logsSubscribed, setLogsSubscribed] = useState(false);
  const logListRef = useRef<HTMLDivElement>(null);
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
    auto_connect: true,
  });
  const [mcpActionLoading, setMcpActionLoading] = useState<string>("");
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
      transport.listSessions(),
      transport.listCrons(),
      transport.listSkills(),
      transport.listModelPresets(),
      transport.listMcpServers(),
    ])
      .then(([cfg, mdl, reg, sess, cronRes, skillRes, presetRes, mcpRes]) => {
        setConfig(cfg as SyscityConfig);
        setModels(mdl.models || []);
        const registry = reg.agents || [];
        setAgentRegistry(registry);
        setSessions(sess || []);
        setCrons(cronRes.jobs || []);
        setSkills(skillRes.skills || []);
        setModelPresets(presetRes || []);
        setMcpServers(mcpRes.servers || []);
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
    const ok = await transport.addMcpServer({
      id: newMcp.id.trim(),
      transport: newMcp.transport,
      command: newMcp.command.trim() || undefined,
      args: newMcp.args.split(",").map((s) => s.trim()).filter(Boolean),
      url: newMcp.url.trim() || undefined,
      auto_connect: newMcp.auto_connect,
    });
    if (ok) {
      setNewMcp({ id: "", transport: "stdio", command: "", args: "", url: "", auto_connect: true });
      setShowAddMcp(false);
      await refreshMcp();
    } else {
      setAddMcpError("Failed to add MCP server");
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
    { id: "channels", label: "Channels" },
    { id: "models", label: "Models" },
    { id: "agents", label: "Agents" },
    { id: "mcp", label: "MCP" },
    { id: "jobs", label: "Jobs" },
    { id: "sessions", label: "Sessions" },
    { id: "skills", label: "Skills" },
    { id: "tools", label: "Tools" },
    { id: "logs", label: "Logs" },
  ];

  const tabCls = (id: string) =>
    `w-full text-left px-3 py-1.5 rounded-md text-sm transition ${
      activeTab === id
        ? "bg-primary-50 dark:bg-primary-900/20 text-primary-700 dark:text-primary-400 font-medium"
        : "text-gray-600 dark:text-neutral-400 hover:bg-gray-100 dark:hover:bg-neutral-800"
    }`;

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-white dark:bg-neutral-900">
      {/* Header */}
      <div className="flex items-center justify-between px-5 py-3 border-b border-gray-200 dark:border-neutral-800 shrink-0">
        <h2 className="text-base font-semibold text-gray-900 dark:text-gray-100">Settings</h2>
        <button
          onClick={onClose}
          className="p-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-neutral-800 text-gray-400 dark:text-neutral-400 transition"
          title="Back to chat"
        >
          <X className="w-5 h-5" />
        </button>
      </div>

      {loading ? (
        <div className="flex-1 flex items-center justify-center text-gray-400 dark:text-neutral-500">
          <div className="w-6 h-6 border-2 border-gray-200 dark:border-neutral-600 border-t-primary-500 rounded-full animate-spin mb-3 mr-3" />
          Loading configuration...
        </div>
      ) : (
        <div className="flex-1 flex overflow-hidden">
          {/* Left vertical tabs */}
          <div className="w-44 border-r border-gray-200 dark:border-neutral-800 shrink-0 overflow-y-auto py-3 px-2 space-y-0.5">
            {tabs.map((t) => (
              <button key={t.id} onClick={() => setActiveTab(t.id)} className={tabCls(t.id)}>
                {t.label}
              </button>
            ))}
          </div>

          {/* Right content */}
          <div className="flex-1 overflow-y-auto px-5 py-4">
            {activeTab === "general" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Gateway</h3>
                  <div className="space-y-2">
                    {(() => {
                      const si = transport.getServerInfo();
                      return (
                        <>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">URL</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono">{transport.getGatewayUrl() || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Version</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono">{si.version || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Connection</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono">{si.conn_id || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Features</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100">{(si.features || []).join(", ") || "—"}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Auth Mode</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-mono capitalize">{String((config as Record<string, unknown>).auth_mode || "—")}</span>
                          </div>
                          <div className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <span className="text-sm text-gray-500 dark:text-neutral-400">Scopes</span>
                            <span className="text-sm text-gray-900 dark:text-gray-100">{(si.scopes_granted || []).join(", ") || "—"}</span>
                          </div>
                        </>
                      );
                    })()}
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Model & Provider</h3>
                  <div className="space-y-3">
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Default Model</label>
                      <select value={config.model || ""} onChange={(e) => update("model", e.target.value)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30">
                        {models.map((m) => <option key={m.id} value={m.id}>{m.name} ({m.provider})</option>)}
                      </select>
                    </div>
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Provider</label>
                      <input type="text" value={config.model_provider || ""} readOnly className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-100 dark:bg-neutral-800 px-3 py-2 text-sm text-gray-500 dark:text-neutral-400 cursor-not-allowed" />
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Appearance</h3>
                  <div className="space-y-3">
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Theme Mode</label>
                      <div className="flex gap-2">
                        {(["system", "light", "dark"] as const).map((m) => (
                          <button key={m} onClick={() => { localStorage.setItem("syscity-theme", m); document.documentElement.classList.toggle("dark", m === "dark" || (m === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)); }} className="px-3 py-1.5 rounded-lg border border-gray-200 dark:border-neutral-600 text-sm text-gray-700 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-neutral-800 transition capitalize">
                            {m}
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Heartbeat</h3>
                  <div className="space-y-3">
                    <div className="flex items-center justify-between">
                      <label className="text-sm text-gray-700 dark:text-gray-300">Enable Heartbeat</label>
                      <button onClick={() => update("heartbeat.enabled", !hb.enabled)} className={`relative inline-flex h-5 w-9 items-center rounded-full transition ${hb.enabled ? "bg-primary-500" : "bg-gray-300 dark:bg-neutral-600"}`}>
                        <span className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition ${hb.enabled ? "translate-x-4.5" : "translate-x-0.5"}`} />
                      </button>
                    </div>
                    <div>
                      <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Interval (seconds)</label>
                      <input type="number" value={hb.interval_seconds ?? 300} onChange={(e) => update("heartbeat.interval_seconds", parseInt(e.target.value))} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                    </div>
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Active From</label>
                        <input type="text" value={hb.active_hours_start || ""} onChange={(e) => update("heartbeat.active_hours_start", e.target.value)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                      </div>
                      <div>
                        <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Active To</label>
                        <input type="text" value={hb.active_hours_end || ""} onChange={(e) => update("heartbeat.active_hours_end", e.target.value)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                      </div>
                    </div>
                  </div>
                </section>
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Token Usage</h3>
                  <div className="text-sm text-gray-500 dark:text-neutral-400">Token usage tracking coming soon.</div>
                </section>
              </div>
            )}

            {activeTab === "channels" && (
              <div className="space-y-5">
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">Configured Channels</h3>
                    <button
                      onClick={() => { setShowAddChannel(!showAddChannel); setAddChannelError(""); }}
                      className="px-3 py-1 rounded-md bg-primary-500 hover:bg-primary-600 text-white text-xs font-medium transition"
                    >
                      {showAddChannel ? "Cancel" : "+ Add"}
                    </button>
                  </div>

                  {showAddChannel && (
                    <div className="mb-4 p-4 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 space-y-3">
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Name</label>
                          <input
                            type="text"
                            value={newChannel.name}
                            onChange={(e) => setNewChannel({ ...newChannel, name: e.target.value })}
                            placeholder="my-bot"
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                          />
                        </div>
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Type</label>
                          <select
                            value={newChannel.channel_type}
                            onChange={(e) => setNewChannel({ ...newChannel, channel_type: e.target.value, credentials: {} })}
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
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
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Agent ID (optional)</label>
                          <input
                            type="text"
                            value={newChannel.agent_id}
                            onChange={(e) => setNewChannel({ ...newChannel, agent_id: e.target.value })}
                            placeholder="default"
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                          />
                        </div>
                        <div className="flex items-center gap-2 pt-5">
                          <input
                            id="ch-enabled"
                            type="checkbox"
                            checked={newChannel.enabled}
                            onChange={(e) => setNewChannel({ ...newChannel, enabled: e.target.checked })}
                            className="rounded border-gray-300 text-primary-500 focus:ring-primary-500"
                          />
                          <label htmlFor="ch-enabled" className="text-sm text-gray-700 dark:text-gray-300">Enabled</label>
                        </div>
                      </div>
                      {channelCredentialFields[newChannel.channel_type]?.map((field) => (
                        <div key={field.key}>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">{field.label}</label>
                          <input
                            type={field.type || "text"}
                            value={newChannel.credentials[field.key] || ""}
                            onChange={(e) => setNewChannel({
                              ...newChannel,
                              credentials: { ...newChannel.credentials, [field.key]: e.target.value },
                            })}
                            placeholder={field.label}
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
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
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No channels configured.</div>
                  ) : (
                    <div className="space-y-2">
                      {channels.map((ch) => (
                        <div key={ch.name} className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center gap-3">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{ch.name}</span>
                            <span className="text-xs px-1.5 py-0.5 rounded bg-gray-200 dark:bg-neutral-700 text-gray-600 dark:text-neutral-400 uppercase">{ch.channel_type}</span>
                            {ch.agent_id && (
                              <span className="text-xs text-gray-500 dark:text-neutral-400 font-mono">{ch.agent_id}</span>
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
                                  : "bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400 hover:bg-gray-200 dark:hover:bg-neutral-600"
                              }`}
                            >
                              {channelActionLoading === ch.name ? "..." : ch.enabled ? "Enabled" : "Disabled"}
                            </button>
                            <button
                              onClick={() => handleRemoveChannel(ch.name)}
                              disabled={channelActionLoading === ch.name}
                              className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-gray-400 dark:text-neutral-400 hover:text-red-600 dark:hover:text-red-400 transition"
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
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">Available Models</h3>
                    <button
                      onClick={() => { setShowAddModel(!showAddModel); setAddModelError(""); }}
                      className="px-3 py-1 rounded-md bg-primary-500 hover:bg-primary-600 text-white text-xs font-medium transition"
                    >
                      {showAddModel ? "Cancel" : "+ Add"}
                    </button>
                  </div>

                  {showAddModel && (
                    <div className="mb-4 p-4 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 space-y-3">
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Alias</label>
                          <input
                            type="text"
                            value={newModel.name}
                            onChange={(e) => setNewModel({ ...newModel, name: e.target.value })}
                            placeholder="smart"
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                          />
                        </div>
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Provider</label>
                          <select
                            value={newModel.provider}
                            onChange={(e) => {
                              const provider = e.target.value;
                              const preset = modelPresets.find((p) => p.name === provider);
                              setNewModel({
                                ...newModel,
                                provider,
                                model: preset?.models[0] || "",
                                base_url: preset?.base_url || "",
                              });
                            }}
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                          >
                            {modelPresets.map((p) => (
                              <option key={p.name} value={p.name}>{p.display_name}</option>
                            ))}
                          </select>
                        </div>
                      </div>
                      {(() => {
                        const preset = modelPresets.find((p) => p.name === newModel.provider);
                        const hasModels = preset && preset.models.length > 0;
                        return (
                          <div>
                            <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Model</label>
                            {hasModels ? (
                              <select
                                value={newModel.model}
                                onChange={(e) => setNewModel({ ...newModel, model: e.target.value })}
                                className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                              >
                                {preset.models.map((m) => (
                                  <option key={m} value={m}>{m}</option>
                                ))}
                              </select>
                            ) : (
                              <input
                                type="text"
                                value={newModel.model}
                                onChange={(e) => setNewModel({ ...newModel, model: e.target.value })}
                                placeholder="model-id"
                                className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                              />
                            )}
                          </div>
                        );
                      })()}
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">API Key</label>
                          <input
                            type="password"
                            value={newModel.api_key}
                            onChange={(e) => setNewModel({ ...newModel, api_key: e.target.value })}
                            placeholder="sk-..."
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                          />
                        </div>
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Base URL</label>
                          <input
                            type="text"
                            value={newModel.base_url}
                            onChange={(e) => setNewModel({ ...newModel, base_url: e.target.value })}
                            placeholder={modelPresets.find((p) => p.name === newModel.provider)?.base_url || "https://..."}
                            className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                          />
                        </div>
                      </div>
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
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No models available.</div>
                  ) : (
                    <div className="space-y-2">
                      {models.map((m) => (
                        <div key={m.id} className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center gap-2">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{m.name}</span>
                            <span className="text-xs text-gray-500 dark:text-neutral-400">{m.provider}</span>
                          </div>
                          <div className="flex items-center gap-2">
                            {config.model === m.id ? (
                              <span className="text-xs px-2 py-0.5 rounded-full bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400">Default</span>
                            ) : (
                              <button
                                onClick={() => handleSetDefaultModel(m.id)}
                                disabled={modelActionLoading === `default_${m.id}`}
                                className="text-xs px-2 py-0.5 rounded-full bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400 hover:bg-primary-100 dark:hover:bg-primary-900/30 hover:text-primary-700 dark:hover:text-primary-400 transition"
                              >
                                {modelActionLoading === `default_${m.id}` ? "..." : "Set Default"}
                              </button>
                            )}
                            <button
                              onClick={() => handleRemoveModel(m.id)}
                              disabled={modelActionLoading === m.id}
                              className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-gray-400 dark:text-neutral-400 hover:text-red-600 dark:hover:text-red-400 transition"
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
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Select Agent</h3>
                  {agentRegistry.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No agents in registry.</div>
                  ) : (
                    <select
                      value={selectedAgentId}
                      onChange={(e) => {
                        const id = e.target.value;
                        setSelectedAgentId(id);
                        loadAgentDetail(id);
                      }}
                      className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
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
                  <div className="text-sm text-gray-500 dark:text-neutral-400 flex items-center gap-2">
                    <div className="w-4 h-4 border-2 border-gray-200 dark:border-neutral-600 border-t-primary-500 rounded-full animate-spin" />
                    Loading agent details...
                  </div>
                )}

                {selectedAgentDetail && !agentDetailLoading && (
                  <section className="space-y-4">
                    {/* Agent header */}
                    <div className="flex items-center gap-3">
                      <span className="text-sm font-medium text-gray-900 dark:text-gray-100">
                        {selectedAgentDetail.personality
                          ? String((selectedAgentDetail.personality as Record<string, unknown>).display_name ?? selectedAgentDetail.agent_id)
                          : selectedAgentDetail.agent_id}
                      </span>
                      <span className="text-xs text-gray-400 dark:text-neutral-500 font-mono">({selectedAgentDetail.agent_id})</span>
                    </div>

                    {/* Config */}
                    {selectedAgentDetail.config && (
                      <div>
                        <h4 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Configuration</h4>
                        <div className="grid grid-cols-2 gap-3">
                          <div className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500">Temperature</div>
                            <div className="text-sm text-gray-900 dark:text-gray-100">{typeof (selectedAgentDetail.config as Record<string, unknown>).temperature === "number" ? ((selectedAgentDetail.config as Record<string, unknown>).temperature as number).toFixed(2) : "—"}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500">Max Tokens</div>
                            <div className="text-sm text-gray-900 dark:text-gray-100">{String((selectedAgentDetail.config as Record<string, unknown>).max_tokens ?? "—")}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500">Max Turns</div>
                            <div className="text-sm text-gray-900 dark:text-gray-100">{String((selectedAgentDetail.config as Record<string, unknown>).max_turns ?? "—")}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500">Max Concurrent Tools</div>
                            <div className="text-sm text-gray-900 dark:text-gray-100">{String((selectedAgentDetail.config as Record<string, unknown>).max_concurrent_tools ?? "—")}</div>
                          </div>
                        </div>
                        {"workspace_only" in (selectedAgentDetail.config as Record<string, unknown>) && (
                          <div className="mt-2 px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 flex items-center justify-between">
                            <span className="text-sm text-gray-700 dark:text-gray-300">Workspace Only</span>
                            <span className={`text-xs px-2 py-0.5 rounded-full ${(selectedAgentDetail.config as Record<string, unknown>).workspace_only ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400" : "bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400"}`}>
                              {(selectedAgentDetail.config as Record<string, unknown>).workspace_only ? "Yes" : "No"}
                            </span>
                          </div>
                        )}
                      </div>
                    )}

                    {/* Personality */}
                    {selectedAgentDetail.personality && (
                      <div>
                        <h4 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Personality</h4>
                        <div className="grid grid-cols-2 gap-3">
                          <div className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500">Display Name</div>
                            <div className="text-sm text-gray-900 dark:text-gray-100">{String((selectedAgentDetail.personality as Record<string, unknown>).display_name ?? "—")}</div>
                          </div>
                          <div className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500">Valid</div>
                            <div className="text-sm text-gray-900 dark:text-gray-100">{(selectedAgentDetail.personality as Record<string, unknown>).is_valid ? "Yes" : "No"}</div>
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
                        <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">
                          {hasAgentCfg ? `${selectedAgentDetail!.agent_id} Parameters` : "Global Default Parameters"}
                        </h3>
                        {hasAgentCfg && (
                          <div className="text-[11px] text-gray-400 dark:text-neutral-500 mb-2">Editing individual agent parameters is not yet supported. Changes here affect the global default.</div>
                        )}
                        <div className="space-y-3">
                          <div className="grid grid-cols-2 gap-3">
                            <div>
                              <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Temperature</label>
                              <div className="flex items-center gap-2">
                                <input type="range" min="0" max="2" step="0.1" value={(ac.temperature as number | undefined) ?? 0.7} onChange={(e) => update("default_agent.temperature", parseFloat(e.target.value))} className="flex-1 h-1.5 bg-gray-200 dark:bg-neutral-600 rounded-lg appearance-none cursor-pointer accent-primary-500" />
                                <span className="text-sm text-gray-600 dark:text-neutral-400 w-10 text-right tabular-nums">{((ac.temperature as number | undefined) ?? 0.7).toFixed(2)}</span>
                              </div>
                            </div>
                            <div>
                              <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Max Tokens</label>
                              <input type="number" value={(ac.max_tokens as number | undefined) ?? 2048} onChange={(e) => update("default_agent.max_tokens", parseInt(e.target.value))} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                            </div>
                          </div>
                          <div className="grid grid-cols-2 gap-3">
                            <div>
                              <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Max Turns</label>
                              <input type="number" value={(ac.max_turns as number | undefined) ?? ""} placeholder="Unlimited" onChange={(e) => update("default_agent.max_turns", e.target.value ? parseInt(e.target.value) : null)} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                            </div>
                            <div>
                              <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">Max Concurrent Tools</label>
                              <input type="number" value={(ac.max_concurrent_tools as number | undefined) ?? 5} onChange={(e) => update("default_agent.max_concurrent_tools", parseInt(e.target.value))} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                            </div>
                          </div>
                          <div>
                            <label className="block text-sm text-gray-700 dark:text-gray-300 mb-1">System Prompt</label>
                            <textarea value={(ac.system_prompt as string | undefined) || ""} onChange={(e) => update("default_agent.system_prompt", e.target.value)} className="w-full h-[60vh] rounded-lg border border-gray-200 dark:border-neutral-600 bg-gray-50 dark:bg-neutral-700 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30 resize-none font-mono" />
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
                <section>
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">MCP Servers</h3>
                    <button onClick={() => setShowAddMcp(true)} className="text-xs px-2 py-1 rounded bg-primary-500 text-white hover:bg-primary-600 transition">+ Add</button>
                  </div>
                  {showAddMcp && (
                    <div className="mb-4 p-4 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 space-y-3">
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Server ID</label>
                          <input type="text" value={newMcp.id} onChange={(e) => setNewMcp({ ...newMcp, id: e.target.value })} placeholder="filesystem" className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                        </div>
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Transport</label>
                          <select value={newMcp.transport} onChange={(e) => setNewMcp({ ...newMcp, transport: e.target.value })} className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30">
                            <option value="stdio">stdio</option>
                            <option value="sse">sse</option>
                            <option value="streamable_http">streamable_http</option>
                          </select>
                        </div>
                      </div>
                      {newMcp.transport === "stdio" ? (
                        <>
                          <div>
                            <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Command</label>
                            <input type="text" value={newMcp.command} onChange={(e) => setNewMcp({ ...newMcp, command: e.target.value })} placeholder="npx -y @modelcontextprotocol/server-filesystem" className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                          </div>
                          <div>
                            <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Args (comma-separated)</label>
                            <input type="text" value={newMcp.args} onChange={(e) => setNewMcp({ ...newMcp, args: e.target.value })} placeholder="/home/user/docs" className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                          </div>
                        </>
                      ) : (
                        <div>
                          <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">URL</label>
                          <input type="text" value={newMcp.url} onChange={(e) => setNewMcp({ ...newMcp, url: e.target.value })} placeholder="http://localhost:3000/sse" className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30" />
                        </div>
                      )}
                      <div className="flex items-center gap-2">
                        <input id="mcp-auto" type="checkbox" checked={newMcp.auto_connect} onChange={(e) => setNewMcp({ ...newMcp, auto_connect: e.target.checked })} className="rounded border-gray-300 text-primary-500 focus:ring-primary-500" />
                        <label htmlFor="mcp-auto" className="text-sm text-gray-700 dark:text-gray-300">Auto-connect</label>
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
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No MCP servers configured.</div>
                  ) : (
                    <div className="space-y-2">
                      {mcpServers.map((srv) => (
                        <div key={srv.id} className="flex items-center justify-between px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                          <div className="flex items-center gap-3">
                            <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{srv.id}</span>
                            <span className="text-xs px-1.5 py-0.5 rounded bg-gray-200 dark:bg-neutral-700 text-gray-600 dark:text-neutral-400 uppercase">{srv.transport}</span>
                            {srv.connected ? (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400">connected</span>
                            ) : (
                              <span className="text-xs px-1.5 py-0.5 rounded bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400">disconnected</span>
                            )}
                          </div>
                          <div className="flex items-center gap-2">
                            {srv.connected ? (
                              <button onClick={() => handleDisconnectMcp(srv.id)} disabled={mcpActionLoading === srv.id} className="text-xs px-2 py-0.5 rounded bg-gray-100 dark:bg-neutral-700 text-gray-600 dark:text-neutral-400 hover:bg-gray-200 dark:hover:bg-neutral-600 transition">
                                {mcpActionLoading === srv.id ? "..." : "Disconnect"}
                              </button>
                            ) : (
                              <button onClick={() => handleConnectMcp(srv.id)} disabled={mcpActionLoading === srv.id} className="text-xs px-2 py-0.5 rounded bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400 hover:bg-primary-200 dark:hover:bg-primary-900/50 transition">
                                {mcpActionLoading === srv.id ? "..." : "Connect"}
                              </button>
                            )}
                            <button onClick={() => handleRemoveMcp(srv.id)} disabled={mcpActionLoading === srv.id} className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-gray-400 dark:text-neutral-400 hover:text-red-600 dark:hover:text-red-400 transition" title="Remove">
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
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Jobs ({crons.length})</h3>
                  {crons.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No cron jobs configured.</div>
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
                          <div key={i} className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="flex items-center justify-between">
                              <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{(j.name as string) || "Unnamed"}</span>
                              <span className={`text-xs px-2 py-0.5 rounded-full ${j.enabled ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400" : "bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400"}`}>
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
                                    <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Schedule</span>
                                    <span className="text-xs text-gray-600 dark:text-neutral-300 font-mono">{expr}</span>
                                  </div>
                                );
                              })()}
                              {nextRun && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Next Run</span>
                                  <span className="text-xs text-gray-600 dark:text-neutral-300">{new Date(nextRun).toLocaleString()}</span>
                                </div>
                              )}
                              {lastRun && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Last Run</span>
                                  <span className="text-xs text-gray-600 dark:text-neutral-300">{new Date(lastRun).toLocaleString()}</span>
                                </div>
                              )}
                              <div className="flex items-center gap-2">
                                <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Target</span>
                                {targetType === "shell" ? (
                                  <span className="text-xs px-1.5 py-0.5 rounded bg-gray-200 dark:bg-neutral-700 text-gray-700 dark:text-neutral-300">Shell</span>
                                ) : targetType === "agent" ? (
                                  <span className="text-xs px-1.5 py-0.5 rounded bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-400">Agent</span>
                                ) : (
                                  <span className="text-xs text-gray-500 dark:text-neutral-400">{targetType || "Unknown"}</span>
                                )}
                              </div>
                              {targetType === "agent" && agentId && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Agent</span>
                                  <span className="text-xs text-gray-600 dark:text-neutral-300 font-mono">{agentId}</span>
                                </div>
                              )}
                              {targetType === "shell" && command && (
                                <div className="flex items-start gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Command</span>
                                  <span className="text-xs text-gray-600 dark:text-neutral-300 font-mono break-all">{command}</span>
                                </div>
                              )}
                              {targetType === "agent" && prompt && (
                                <div className="flex items-start gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Prompt</span>
                                  <span className="text-xs text-gray-600 dark:text-neutral-300 line-clamp-2">{prompt}</span>
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

            {activeTab === "sessions" && (
              <div className="space-y-5">
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Active Sessions</h3>
                  {sessions.length === 0 ? (
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No sessions.</div>
                  ) : (
                    <div className="space-y-2">
                      {sessions.map((s) => {
                        const meta = s as unknown as Record<string, unknown>;
                        const agentId = meta.agent_id as string | undefined;
                        const channel = meta.channel as string | undefined;
                        const msgCount = meta.message_count as number | undefined;
                        const lastActivity = meta.last_activity as string | undefined;
                        const isActive = meta.is_active as boolean | undefined;
                        return (
                          <div key={s.id} className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="flex items-center justify-between">
                              <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{s.label || s.id}</span>
                              <div className="flex items-center gap-1.5">
                                {isActive !== undefined && (
                                  <span className={`text-xs px-2 py-0.5 rounded-full ${isActive ? "bg-primary-100 dark:bg-primary-900/30 text-primary-700 dark:text-primary-400" : "bg-gray-100 dark:bg-neutral-700 text-gray-500 dark:text-neutral-400"}`}>
                                    {isActive ? "Active" : "Inactive"}
                                  </span>
                                )}
                                {msgCount !== undefined && (
                                  <span className="text-xs text-gray-500 dark:text-neutral-400">{msgCount} msgs</span>
                                )}
                              </div>
                            </div>
                            <div className="mt-1.5 space-y-1">
                              {agentId && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Agent</span>
                                  <span className="text-xs text-gray-600 dark:text-neutral-300 font-mono">{agentId}</span>
                                </div>
                              )}
                              {channel && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Channel</span>
                                  <span className="text-xs px-1.5 py-0.5 rounded bg-gray-200 dark:bg-neutral-700 text-gray-600 dark:text-neutral-300">{channel}</span>
                                </div>
                              )}
                              {lastActivity && (
                                <div className="flex items-center gap-2">
                                  <span className="text-[10px] uppercase tracking-wider text-gray-400 dark:text-neutral-500 w-16 shrink-0">Last Active</span>
                                  <span className="text-xs text-gray-600 dark:text-neutral-300">{new Date(lastActivity).toLocaleString()}</span>
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
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">Skills ({skills.length})</h3>
                    <button
                      onClick={() => { setShowAddSkill(!showAddSkill); setAddSkillError(""); }}
                      className="px-3 py-1 rounded-md bg-primary-500 hover:bg-primary-600 text-white text-xs font-medium transition"
                    >
                      {showAddSkill ? "Cancel" : "+ Install"}
                    </button>
                  </div>

                  {showAddSkill && (
                    <div className="mb-4 p-4 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800 space-y-3">
                      <div>
                        <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">Skill Name</label>
                        <input
                          type="text"
                          value={newSkillName}
                          onChange={(e) => setNewSkillName(e.target.value)}
                          placeholder="my-skill"
                          className="w-full rounded-lg border border-gray-200 dark:border-neutral-600 bg-white dark:bg-neutral-700 px-3 py-1.5 text-sm text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
                        />
                      </div>
                      <div>
                        <label className="block text-xs text-gray-600 dark:text-neutral-400 mb-1">ZIP File</label>
                        <input
                          type="file"
                          accept=".zip"
                          onChange={(e) => setNewSkillZip(e.target.files?.[0] || null)}
                          className="w-full text-sm text-gray-700 dark:text-neutral-300 file:mr-3 file:py-1.5 file:px-3 file:rounded-md file:border-0 file:text-xs file:font-medium file:bg-primary-50 file:text-primary-700 dark:file:bg-primary-900/20 dark:file:text-primary-400 hover:file:bg-primary-100"
                        />
                        <p className="text-[10px] text-gray-400 dark:text-neutral-500 mt-1">ZIP must contain a SKILL.md file at the root.</p>
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
                    <div className="text-sm text-gray-500 dark:text-neutral-400">No skills loaded.</div>
                  ) : (
                    <div className="space-y-2">
                      {skills.map((s, i) => {
                        const sk = s as Record<string, unknown>;
                        const triggers = (sk.triggers as Array<Record<string, unknown>>) || [];
                        const deps = sk.depends_on as Record<string, string> | undefined;
                        const provides = (sk.provides as string[]) || [];
                        const chain = (sk.chain as string[]) || [];
                        return (
                          <div key={i} className="px-3 py-2 rounded-lg border border-gray-200 dark:border-neutral-700 bg-gray-50 dark:bg-neutral-800">
                            <div className="flex items-center justify-between">
                              <div className="flex items-center gap-2">
                                <span className="text-sm text-gray-900 dark:text-gray-100 font-medium">{String(sk.name || "Unnamed")}</span>
                                <span className="text-xs text-gray-500 dark:text-neutral-400">{String(sk.version || "")}</span>
                              </div>
                              {Boolean(sk.author) && (
                                <span className="text-xs text-gray-400 dark:text-neutral-500">by {String(sk.author)}</span>
                              )}
                            </div>
                            {Boolean(sk.description) && (
                              <div className="text-xs text-gray-500 dark:text-neutral-400 mt-1">{String(sk.description)}</div>
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
                              <div className="mt-1 text-[10px] text-gray-400 dark:text-neutral-500">
                                deps: {Object.entries(deps).map(([k, v]) => `${k}@${v}`).join(", ")}
                              </div>
                            )}
                            {chain.length > 0 && (
                              <div className="mt-1 text-[10px] text-gray-400 dark:text-neutral-500">
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
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Default Search Provider</h3>
                  <select
                    value={config.search?.provider ?? "duckduckgo"}
                    onChange={(e) => update("search.provider", e.target.value)}
                    className="w-full text-sm border border-gray-200 dark:border-neutral-700 rounded-lg px-3 py-2 bg-white dark:bg-neutral-800 text-gray-900 dark:text-gray-100"
                  >
                    {SEARCH_PROVIDERS.map((p) => (
                      <option key={p.id} value={p.id}>{p.label}</option>
                    ))}
                  </select>
                </section>

                {/* Fallback Provider Order */}
                <section>
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Fallback Order</h3>
                  <div className="flex flex-wrap gap-2 mb-2">
                    {(config.search?.providers ?? []).map((prov) => (
                      <span key={prov} className="inline-flex items-center gap-1 px-2.5 py-1 rounded-full text-xs bg-gray-100 dark:bg-neutral-800 text-gray-700 dark:text-neutral-300">
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
                      className="text-sm border border-gray-200 dark:border-neutral-700 rounded-lg px-3 py-2 bg-white dark:bg-neutral-800 text-gray-900 dark:text-gray-100"
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
                  <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider mb-2">Provider API Keys</h3>
                  <div className="space-y-3">
                    {SEARCH_PROVIDERS.filter((p) => p.needsKey).map((p) => (
                      <div key={p.id} className="flex items-center gap-3">
                        <label className="w-28 text-sm text-gray-700 dark:text-neutral-300 shrink-0">{p.label}</label>
                        <input
                          type="password"
                          placeholder={config.search?.keys?.[p.id] === "true" ? "••••••••" : ""}
                          value=""
                          onChange={(e) => update(`search.keys.${p.id}`, e.target.value)}
                          onFocus={(e) => (e.target.value = "")}
                          className="flex-1 text-sm border border-gray-200 dark:border-neutral-700 rounded-lg px-3 py-2 bg-white dark:bg-neutral-800 text-gray-900 dark:text-gray-100 placeholder-gray-400"
                        />
                      </div>
                    ))}
                    {/* Google CX special case */}
                    <div className="flex items-center gap-3">
                      <label className="w-28 text-sm text-gray-700 dark:text-neutral-300 shrink-0">Google CX</label>
                      <input
                        type="password"
                        placeholder={config.search?.keys?.google_cx === "true" ? "••••••••" : ""}
                        value=""
                        onChange={(e) => update("search.keys.google_cx", e.target.value)}
                        onFocus={(e) => (e.target.value = "")}
                        className="flex-1 text-sm border border-gray-200 dark:border-neutral-700 rounded-lg px-3 py-2 bg-white dark:bg-neutral-800 text-gray-900 dark:text-gray-100 placeholder-gray-400"
                      />
                    </div>
                  </div>
                </section>
              </div>
            )}

            {activeTab === "logs" && (
              <div className="flex flex-col h-full">
                <section className="flex flex-col flex-1">
                  <div className="flex items-center justify-between mb-2">
                    <h3 className="text-xs font-semibold text-gray-500 dark:text-neutral-400 uppercase tracking-wider">Logs</h3>
                    <span className={`text-[10px] px-2 py-0.5 rounded-full ${logsSubscribed ? 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-400' : 'bg-gray-100 text-gray-500 dark:bg-neutral-800 dark:text-neutral-500'}`}>
                      {logsSubscribed ? "Live" : "Disconnected"}
                    </span>
                  </div>
                  <div
                    ref={logListRef}
                    className="bg-gray-50 dark:bg-neutral-900 rounded-lg border border-gray-200 dark:border-neutral-700 h-[90vh] overflow-y-auto font-mono text-[11px] leading-4 p-3"
                  >
                    {logLines.length === 0 && (
                      <div className="text-gray-400 dark:text-neutral-600 text-center py-20">
                        {logsSubscribed ? "Waiting for logs..." : "Click the Logs tab to connect"}
                      </div>
                    )}
                    {logLines.map((line, i) => (
                      <div key={i} className="text-gray-700 dark:text-neutral-300 whitespace-pre-wrap break-all py-0.5 border-b border-gray-100 dark:border-neutral-800/50 last:border-0">
                        {line}
                      </div>
                    ))}
                  </div>
                  <div className="flex gap-2 mt-2">
                    <button
                      onClick={() => setLogLines([])}
                      className="px-3 py-1.5 text-xs bg-gray-100 hover:bg-gray-200 dark:bg-neutral-800 dark:hover:bg-neutral-700 rounded-md text-gray-600 dark:text-neutral-400 transition-colors"
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
                      className="px-3 py-1.5 text-xs bg-gray-100 hover:bg-gray-200 dark:bg-neutral-800 dark:hover:bg-neutral-700 rounded-md text-gray-600 dark:text-neutral-400 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
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
    </div>
  );
}
