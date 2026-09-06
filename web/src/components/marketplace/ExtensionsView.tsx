import { MarketplaceSettings } from "@/components/settings/MarketplaceSettings";

/** Full-screen Extensions page (the marketplace: connectors, skills,
 * experts) that replaces the chat area when opened from the sidebar.
 * Reuses the same MarketplaceSettings content as the settings tab, but as
 * its own page — the "Extensions" title lives in the app Titlebar (via the
 * `page` slot), aligned with this page's content padding. `initialType`
 * pre-filters the catalog (connector/skill/expert). `onSummonExpert` is
 * forwarded so summoning an expert opens a new session with its agent. */
export function ExtensionsView({
  initialType,
  onSummonExpert,
}: {
  initialType?: string | null;
  onSummonExpert?: (agentId: string) => void;
}) {
  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-page">
      <div className="flex-1 overflow-y-auto px-6 md:px-8 py-4">
        <MarketplaceSettings initialType={initialType ?? undefined} onSummonExpert={onSummonExpert} />
      </div>
    </div>
  );
}
