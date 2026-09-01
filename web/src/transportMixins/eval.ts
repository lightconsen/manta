import type { SyscityWebSocketTransport } from "../transportCore";
import type {
  EvalDashboardPayload,
  FeedbackOpsPayload,
} from "../transportTypes";

// Domain mixin: eval RPC method implementations (installed on the prototype by
// the facade `SyscityWebSocketTransport.ts`). Signatures are merged onto the
// class type in `transportCore.ts`.
export function install(proto: typeof SyscityWebSocketTransport.prototype): void {
  proto.getEvalDashboard = async function (this: SyscityWebSocketTransport,): Promise<EvalDashboardPayload> {
    try {
      const res = (await this.sendRequestAndWait(
        "eval.dashboard",
        {},
        8000
      )) as EvalDashboardPayload | undefined;
      if (!res) return this.emptyEvalDashboard();
      return {
        traces: {
          total: res.traces?.total ?? 0,
          by_kind: res.traces?.by_kind ?? {},
          by_status: res.traces?.by_status ?? {},
          recent: res.traces?.recent ?? [],
        },
        badcases: {
          total: res.badcases?.total ?? 0,
          by_source: res.badcases?.by_source ?? {},
          by_status: res.badcases?.by_status ?? {},
        },
        feedback: {
          since_ms: res.feedback?.since_ms ?? 0,
          up: res.feedback?.up ?? 0,
          down: res.feedback?.down ?? 0,
          total: res.feedback?.total ?? 0,
        },
        trends: res.trends ?? [],
        optimizer: {
          running: !!res.optimizer?.running,
          paused: !!res.optimizer?.paused,
          breaker: {
            failures: res.optimizer?.breaker?.failures ?? 0,
            tripped: !!res.optimizer?.breaker?.tripped,
            open: !!res.optimizer?.breaker?.open,
          },
          last_run_at: res.optimizer?.last_run_at ?? null,
          last_report: res.optimizer?.last_report ?? null,
          last_error: res.optimizer?.last_error ?? null,
        },
      };
    } catch {
      return this.emptyEvalDashboard();
    }
  };
  proto.emptyEvalDashboard = function (this: SyscityWebSocketTransport,): EvalDashboardPayload {
    return {
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
  };
  proto.getFeedbackOps = async function (this: SyscityWebSocketTransport,): Promise<FeedbackOpsPayload> {
    try {
      const res = (await this.sendRequestAndWait(
        "feedback.ops",
        {},
        8000
      )) as FeedbackOpsPayload | undefined;
      if (!res) return this.emptyFeedbackOps();
      return {
        since_ms: res.since_ms ?? 0,
        total_votes: res.total_votes ?? 0,
        up: res.up ?? 0,
        down: res.down ?? 0,
        by_agent: res.by_agent ?? [],
        pending_by_source: res.pending_by_source ?? [],
        by_day: res.by_day ?? [],
        down_votes: res.down_votes ?? [],
        risk_clusters: res.risk_clusters ?? [],
      };
    } catch {
      return this.emptyFeedbackOps();
    }
  };
  proto.emptyFeedbackOps = function (this: SyscityWebSocketTransport,): FeedbackOpsPayload {
    return {
      since_ms: 0,
      total_votes: 0,
      up: 0,
      down: 0,
      by_agent: [],
      pending_by_source: [],
      by_day: [],
      down_votes: [],
      risk_clusters: [],
    };
  };
}
