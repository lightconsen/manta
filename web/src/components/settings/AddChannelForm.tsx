import { useState } from "react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { Input } from "@/components/ui/Input";
import { Select } from "@/components/ui/Select";
import { Button } from "@/components/ui/Button";

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

interface AddChannelFormProps {
  transport: SyscityWebSocketTransport;
  onAdded?: () => void;
}

export function AddChannelForm({ transport, onAdded }: AddChannelFormProps) {
  const [addChannelError, setAddChannelError] = useState("");
  const [newChannel, setNewChannel] = useState({
    name: "",
    channel_type: "telegram",
    enabled: true,
    agent_id: "",
    credentials: {} as Record<string, string>,
  });
  const [channelActionLoading, setChannelActionLoading] = useState<string>("");

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
      onAdded?.();
    } else {
      setAddChannelError("Failed to add channel");
    }
    setChannelActionLoading("");
  };

  return (
    <div className="mb-4 p-4 rounded-lg bg-card space-y-3">
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <Input
          label="Name"
          value={newChannel.name}
          onChange={(e) => setNewChannel({ ...newChannel, name: e.target.value })}
          placeholder="my-bot"
        />
        <div>
          <label className="block text-xs text-secondary mb-1">Type</label>
          <Select
            value={newChannel.channel_type}
            onChange={(e) => setNewChannel({ ...newChannel, channel_type: e.target.value, credentials: {} })}
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
          </Select>
        </div>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <Input
          label="Agent ID (optional)"
          value={newChannel.agent_id}
          onChange={(e) => setNewChannel({ ...newChannel, agent_id: e.target.value })}
          placeholder="default"
        />
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
        <Input
          key={field.key}
          label={field.label}
          type={field.type || "text"}
          value={newChannel.credentials[field.key] || ""}
          onChange={(e) =>
            setNewChannel({
              ...newChannel,
              credentials: { ...newChannel.credentials, [field.key]: e.target.value },
            })
          }
          placeholder={field.label}
        />
      ))}
      {addChannelError && (
        <div className="text-xs text-red-600 dark:text-red-400">{addChannelError}</div>
      )}
      <div className="flex justify-end">
        <Button onClick={handleAddChannel} disabled={channelActionLoading === "add"}>
          {channelActionLoading === "add" ? "Adding..." : "Add Channel"}
        </Button>
      </div>
    </div>
  );
}
