import { useState } from "react";
import type { SyscityWebSocketTransport } from "@/SyscityWebSocketTransport";
import { AddSkillForm } from "@/components/settings/AddSkillForm";
import { Button } from "@/components/ui/Button";

interface SkillsSettingsProps {
  transport: SyscityWebSocketTransport;
  skills: Array<Record<string, unknown>>;
  onRefresh: () => Promise<void>;
}

export function SkillsSettings({ transport, skills, onRefresh }: SkillsSettingsProps) {
  const [showAddSkill, setShowAddSkill] = useState(false);

  return (
    <div className="space-y-5">
      <section>
        <div className="flex items-center justify-between mb-2">
          <h3 className="text-xs font-semibold text-secondary uppercase tracking-wider">Skills ({skills.length})</h3>
          <Button variant="primary-sm" onClick={() => setShowAddSkill(!showAddSkill)}>
            {showAddSkill ? "Cancel" : "+ Install"}
          </Button>
        </div>

        {showAddSkill && (
          <AddSkillForm
            transport={transport}
            onAdded={() => {
              setShowAddSkill(false);
              onRefresh();
            }}
          />
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
  );
}
