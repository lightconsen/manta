import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import type {
  EvalDashboardPayload,
  SyscityWebSocketTransport,
} from "@/SyscityWebSocketTransport";
import { Section } from "@/components/ui/Section";

interface EvalDashboardProps {
  transport: SyscityWebSocketTransport;
}

const EMPTY: EvalDashboardPayload = {
  traces: { total: 0, by_kind: {}, by_status: {}, recent: [] },
  badcases: { total: 0, by_source: {}, by_status: {} },
  feedback: { since_ms: 0, up: 0, down: 0, total: 0 },
  trends: [],
  optimizer: {
    running: false,
    paused: false,
    breaker: { failures: 0, tripped: false, open: false },
    last_run_at: null,
    last_report: null,
    last_error: null,
  },
};

function fmtTime(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || ms <= 0) return "—";
  const d = new Date(ms);
  if (isNaN(d.getTime())) return "—";
  return d.toLocaleString();
}

function Badge({
  className,
  children,
}: {
  className: string;
  children: ReactNode;
}) {
  return (
    <span className={`px-1.5 py-0.5 rounded-full text-xs ${className}`}>
      {children}
    </span>
  );
}

/** A single mini bar+count cell for the daily trend rows. */
function TrendCell({
  value,
  max,
  color,
}: {
  value: number;
  max: number;
  color: string;
}) {
  const pct = value === 0 ? 0 : max > 0 ? Math.max(8, Math.round((value / max) * 100)) : 0;
  return (
    <div className="flex items-center gap-2">
      <div className="h-1.5 flex-1 min-w-[40px] rounded bg-black/5 dark:bg-white/10 overflow-hidden">
        <div className={`h-full rounded ${color}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="text-xs text-secondary tabular-nums w-6 text-right">{value}</span>
    </div>
  );
}

/** Read-only eval dashboard (§八 评测看板). Rendered as the "Eval" settings tab. */
export function EvalDashboard({ transport }: EvalDashboardProps) {
  const [data, setData] = useState<EvalDashboardPayload>(EMPTY);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    setLoading(true);
    try {
      const res = await transport.getEvalDashboard();
      setData(res);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
  }, [transport]);

  if (loading && data === EMPTY) {
    return (
      <div className="flex items-center justify-center text-secondary h-40">
        <div className="w-6 h-6 border-2 border-subtle border-t-primary-500 rounded-full animate-spin mr-3" />
        Loading eval dashboard...
      </div>
    );
  }

  const opt = data.optimizer;
  const report = opt.last_report;
  const applied = report?.applied?.length ?? 0;
  const rejected = report?.rejected?.length ?? 0;
  const statCls = "px-3 py-2 rounded-lg bg-card";
  const maxVals = {
    up: Math.max(1, ...data.trends.map((t) => t.up)),
    down: Math.max(1, ...data.trends.map((t) => t.down)),
    badcases: Math.max(1, ...data.trends.map((t) => t.badcases)),
    traces: Math.max(1, ...data.trends.map((t) => t.traces)),
  };

  return (
    <div className="space-y-5">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold text-primary">Eval Dashboard</h2>
        <button
          onClick={refresh}
          disabled={loading}
          className="px-3 py-1.5 rounded-lg text-sm text-secondary border border-subtle hover:bg-black/[0.03] dark:hover:bg-white/[0.04] transition disabled:opacity-50"
        >
          {loading ? "Refreshing..." : "Refresh"}
        </button>
      </div>

      {error && (
        <div className="text-sm text-red-600 dark:text-red-400 px-3 py-2 rounded-lg bg-red-50 dark:bg-red-900/20">
          Failed to load eval dashboard: {error}
        </div>
      )}

      {/* Optimizer runtime + guardrails */}
      <Section title="Optimizer">
        <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
          <div className={statCls}>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Status</div>
            <div className="mt-1 flex items-center gap-1.5 flex-wrap">
              {opt.running ? (
                <Badge className="bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400">
                  Running
                </Badge>
              ) : (
                <Badge className="bg-sidebar text-secondary">Idle</Badge>
              )}
              {opt.paused && (
                <Badge className="bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400">
                  Paused
                </Badge>
              )}
            </div>
          </div>
          <div className={statCls}>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Last Run</div>
            <div className="mt-1 text-sm text-secondary">{fmtTime(opt.last_run_at)}</div>
          </div>
          <div className={statCls}>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Circuit Breaker</div>
            <div className="mt-1 flex items-center gap-1.5 flex-wrap">
              <Badge
                className={
                  opt.breaker.open
                    ? "bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400"
                    : "bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400"
                }
              >
                {opt.breaker.open ? "Open" : "Closed"}
              </Badge>
              <span className="text-xs text-secondary">failures: {opt.breaker.failures}</span>
            </div>
          </div>
          <div className={statCls}>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Last Report</div>
            <div className="mt-1 text-sm text-secondary line-clamp-2">
              {report ? `${report.reason} (applied ${applied}, rejected ${rejected})` : "—"}
            </div>
          </div>
        </div>
        {opt.last_error && (
          <div className="mt-2 text-xs text-red-600 dark:text-red-400 break-all">{opt.last_error}</div>
        )}
      </Section>

      {/* Decision traces */}
      <Section title="Decision Traces">
        <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
          <div className={statCls}>
            <div className="text-2xl font-semibold text-primary">{data.traces.total}</div>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Total</div>
          </div>
          {Object.entries(data.traces.by_kind).map(([k, v]) => (
            <div key={k} className={statCls}>
              <div className="text-2xl font-semibold text-primary">{v}</div>
              <div className="text-[10px] uppercase tracking-wider text-secondary/70">{k}</div>
            </div>
          ))}
        </div>
        <div className="mt-2 flex flex-wrap gap-1.5">
          {Object.entries(data.traces.by_status).map(([k, v]) => (
            <span key={k} className="text-xs px-2 py-0.5 rounded-full bg-sidebar text-secondary">
              {k}: {v}
            </span>
          ))}
        </div>
      </Section>

      {/* Badcases */}
      <Section title="Badcases">
        <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
          <div className={statCls}>
            <div className="text-2xl font-semibold text-primary">{data.badcases.total}</div>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Total</div>
          </div>
          {Object.entries(data.badcases.by_source).map(([k, v]) => (
            <div key={k} className={statCls}>
              <div className="text-2xl font-semibold text-primary">{v}</div>
              <div className="text-[10px] uppercase tracking-wider text-secondary/70">{k}</div>
            </div>
          ))}
        </div>
        <div className="mt-2 flex flex-wrap gap-1.5">
          {Object.entries(data.badcases.by_status).map(([k, v]) => (
            <span key={k} className="text-xs px-2 py-0.5 rounded-full bg-sidebar text-secondary">
              {k}: {v}
            </span>
          ))}
        </div>
      </Section>

      {/* Feedback */}
      <Section title="Feedback (30d)">
        <div className="grid grid-cols-3 gap-3">
          <div className={statCls}>
            <div className="text-2xl font-semibold text-primary">{data.feedback.up}</div>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Likes</div>
          </div>
          <div className={statCls}>
            <div className="text-2xl font-semibold text-primary">{data.feedback.down}</div>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Dislikes</div>
          </div>
          <div className={statCls}>
            <div className="text-2xl font-semibold text-primary">{data.feedback.total}</div>
            <div className="text-[10px] uppercase tracking-wider text-secondary/70">Total</div>
          </div>
        </div>
      </Section>

      {/* Trends (14d) */}
      <Section title="Trends (14d)">
        {data.trends.length === 0 ? (
          <div className="text-sm text-secondary">No trend data yet.</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-left text-[10px] uppercase tracking-wider text-secondary/70">
                  <th className="py-1.5 pr-3 font-medium">Day</th>
                  <th className="py-1.5 pr-3 font-medium">Likes</th>
                  <th className="py-1.5 pr-3 font-medium">Dislikes</th>
                  <th className="py-1.5 pr-3 font-medium">Badcases</th>
                  <th className="py-1.5 font-medium">Traces</th>
                </tr>
              </thead>
              <tbody>
                {data.trends.map((t) => (
                  <tr key={t.day} className="border-t border-subtle">
                    <td className="py-1.5 pr-3 font-mono text-xs text-secondary whitespace-nowrap">
                      {t.day}
                    </td>
                    <td className="py-1.5 pr-3">
                      <TrendCell value={t.up} max={maxVals.up} color="bg-green-500" />
                    </td>
                    <td className="py-1.5 pr-3">
                      <TrendCell value={t.down} max={maxVals.down} color="bg-red-500" />
                    </td>
                    <td className="py-1.5 pr-3">
                      <TrendCell value={t.badcases} max={maxVals.badcases} color="bg-amber-500" />
                    </td>
                    <td className="py-1.5">
                      <TrendCell value={t.traces} max={maxVals.traces} color="bg-sky-500" />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Section>

      {/* Recent traces */}
      <Section title={`Recent Traces (${data.traces.recent.length})`}>
        {data.traces.recent.length === 0 ? (
          <div className="text-sm text-secondary">No decision traces yet.</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-left text-[10px] uppercase tracking-wider text-secondary/70">
                  <th className="py-1.5 pr-2 font-medium">Kind</th>
                  <th className="py-1.5 pr-2 font-medium">Subject</th>
                  <th className="py-1.5 pr-2 font-medium">Status</th>
                  <th className="py-1.5 font-medium">Decided At</th>
                </tr>
              </thead>
              <tbody>
                {data.traces.recent.map((t) => (
                  <tr key={t.id} className="border-t border-subtle">
                    <td className="py-1.5 pr-2 font-mono text-xs text-primary whitespace-nowrap">{t.kind}</td>
                    <td className="py-1.5 pr-2 text-secondary break-all">{t.subject}</td>
                    <td className="py-1.5 pr-2">
                      <Badge className="bg-sidebar text-secondary">{t.status}</Badge>
                    </td>
                    <td className="py-1.5 text-secondary whitespace-nowrap">{fmtTime(t.decided_at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Section>
    </div>
  );
}
