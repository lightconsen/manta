// Syscity Cloud session helpers.
//
// These run over WebSocket admin methods (cloud.status, cloud.subscription,
// cloud.usage, cloud.token, cloud.logout) via the active transport — the
// built-in UI is WS-only. cloudLoginUrl is a browser redirect (HTTP) and is
// the one cloud path that always uses HTTP.

import { getGatewayBase } from "./gatewayBase";
import { getActiveTransport } from "@/SyscityWebSocketTransport";

export interface CloudStatus {
  enabled: boolean;
  logged_in: boolean;
  user: { id?: string; name?: string; email?: string | null } | null;
}

export interface CloudSubscription {
  plan: string;
  plan_rank: number;
  status: string;
  balance: number;
  overdrawn: boolean;
  threshold_warn: boolean;
  period_end: string | null;
}

export interface CloudUsage {
  days: number;
  month_credits: number;
  total_calls: number;
  total_credits: number;
  by_model: Array<{ model: string; calls: number; credits: number }>;
  by_category: Array<{ category: string; calls: number; credits: number }>;
}

/** Cloud status — `null` in default (non-cloud) builds. */
export async function cloudStatus(): Promise<CloudStatus> {
  const transport = getActiveTransport();
  if (!transport) throw new Error("No gateway connection");
  const body = (await transport.getCloudStatus()) as CloudStatus | null;
  if (!body) return { enabled: false, logged_in: false, user: null };
  return body;
}

/** Plan + credit balance (+ low-credit/overdraft flags). */
export async function cloudSubscription(): Promise<CloudSubscription> {
  const transport = getActiveTransport();
  if (!transport) throw new Error("No gateway connection");
  return (await transport.getCloudSubscription()) as CloudSubscription;
}

/** Credit usage for the last `days` (default 30). */
export async function cloudUsage(days = 30): Promise<CloudUsage> {
  const transport = getActiveTransport();
  if (!transport) throw new Error("No gateway connection");
  return (await transport.getCloudUsage(days)) as CloudUsage;
}

/** The cloud OAuth login URL (engine route that 302s to the cloud). */
export function cloudLoginUrl(provider = "github"): string {
  return `${getGatewayBase()}/api/v1/cloud/login?provider=${provider}`;
}

/** Redirect the current tab to the cloud OAuth login (welcome-page flow). */
export function cloudLogin(provider = "github") {
  window.location.href = cloudLoginUrl(provider);
}

/** Persist a session token returned by the cloud OAuth callback. */
export async function cloudSubmitToken(token: string): Promise<boolean> {
  const transport = getActiveTransport();
  if (!transport) return false;
  try {
    const r = (await transport.submitCloudToken(token)) as { ok?: boolean };
    return r?.ok ?? true;
  } catch {
    return false;
  }
}

/** Forget the cloud session token. */
export async function cloudLogout(): Promise<void> {
  const transport = getActiveTransport();
  if (!transport) return;
  await transport.cloudLogout();
}
