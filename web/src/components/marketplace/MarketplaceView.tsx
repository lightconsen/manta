import { X } from "lucide-react";
import { MarketplaceSettings } from "@/components/settings/MarketplaceSettings";

/** Full-screen marketplace view that replaces the chat area when opened from
 * the sidebar. Reuses the same MarketplaceSettings content as the settings
 * tab, but as its own page (own header + back to chat). `initialType` pre-
 * filters the catalog (connector/skill/expert) so sidebar entries can open
 * one section directly. `onSummonExpert` is forwarded so summoning an expert
 * opens a new session with its agent. */
export function MarketplaceView({
  initialType,
  onClose,
  onSummonExpert,
}: {
  initialType?: string | null;
  onClose: () => void;
  onSummonExpert?: (agentId: string) => void;
}) {
  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-page">
      <div className="flex items-center justify-between px-4 md:px-5 py-3 border-b border-subtle shrink-0">
        <h2 className="text-base font-semibold text-primary">Marketplace</h2>
        <button
          onClick={onClose}
          className="p-1.5 rounded-lg hover:bg-black/5 dark:hover:bg-white/5 text-secondary transition"
          title="Back to chat"
          aria-label="Back to chat"
        >
          <X className="w-5 h-5" />
        </button>
      </div>
      <div className="flex-1 overflow-y-auto px-4 md:px-5 py-4">
        <MarketplaceSettings initialType={initialType ?? undefined} onSummonExpert={onSummonExpert} />
      </div>
    </div>
  );
}
