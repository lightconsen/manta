import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { AddModelForm } from "@/components/settings/AddModelForm";

interface WelcomeScreenProps {
  transport: SyscityWebSocketTransport;
  onComplete: () => void;
}

export function WelcomeScreen({ transport, onComplete }: WelcomeScreenProps) {
  return (
    <div
      className="flex overflow-y-auto bg-page text-primary"
      style={{
        paddingTop: "env(safe-area-inset-top)",
        paddingBottom: "env(safe-area-inset-bottom)",
        minHeight: "100lvh",
      }}
    >
      {/* m-auto centers when there is room; with overflow the margins collapse
          to 0 so the form stays reachable (and scrollable) on small phones. */}
      <div className="m-auto w-full max-w-2xl px-6 py-8">
        <div className="flex flex-col items-center mb-8">
          <img src="/syscity.png" alt="Syscity" className="w-24 h-24 object-contain mb-6" />
          <h1 className="text-3xl font-semibold mb-2">Welcome to Syscity</h1>
          <p className="text-secondary text-sm">
            Configure your first LLM model to get started.
          </p>
        </div>
        <div className="rounded-lg bg-card border border-subtle p-2">
          <AddModelForm transport={transport} onAdded={onComplete} />
        </div>
      </div>
    </div>
  );
}
