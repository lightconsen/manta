interface McpAuthModalProps {
  authModal: { serverId: string; authUrl: string } | null;
  onCancel: () => Promise<void>;
}

export function McpAuthModal({ authModal, onCancel }: McpAuthModalProps) {
  if (!authModal) return null;
  return (
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
            onClick={() => onCancel()}
            className="px-3 py-1.5 text-xs font-medium rounded-lg bg-sidebar text-secondary hover:text-primary transition-colors"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
