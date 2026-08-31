// Syscity Cloud session helpers.
//
// In the built-in UI these run over WebSocket admin methods (cloud.status,
// cloud.subscription, cloud.usage, cloud.token, cloud.logout) via the active
// transport. The REST surface stays for external tools / CLI; cloudLoginUrl
// is a browser redirect and always uses HTTP.

import { apiFetch, getGatewayBase } from "./gatewayBase";
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

/** Engine status — the cloud block is `null` in default (non-cloud) builds. */
export async function cloudStatus(): Promise<CloudStatus> {
  const transport = getActiveTransport();
  if (transport) {
    const body = (await transport.getCloudStatus()) as {
      cloud?: CloudStatus | null;
    };
    const cloud = body?.cloud;
    if (!cloud) return { enabled: false, logged_in: false, user: null };
    return cloud;
  }
  const res = await apiFetch("/api/v1/status");
  if (!res.ok) throw new Error(`status: HTTP ${res.status}`);
  const body = await res.json();
  const cloud = body.cloud as CloudStatus | null | undefined;
  if (!cloud) return { enabled: false, logged_in: false, user: null };
  return cloud;
}

/** Plan + credit balance (+ low-credit/overdraft flags). */
export async function cloudSubscription(): Promise<CloudSubscription> {
  const transport = getActiveTransport();
  if (transport) {
    return (await transport.getCloudSubscription()) as CloudSubscription;
  }
  const res = await apiFetch("/api/v1/cloud/subscription");
  if (!res.ok) throw new Error(`subscription: HTTP ${res.status}`);
  return res.json();
}

/** Credit usage for the last `days` (default 30). */
export async function cloudUsage(days = 30): Promise<CloudUsage> {
  const transport = getActiveTransport();
  if (transport) {
    return (await transport.getCloudUsage(days)) as CloudUsage;
  }
  const res = await apiFetch(`/api/v1/cloud/usage?days=${days}`);
  if (!res.ok) throw new Error(`usage: HTTP ${res.status}`);
  return res.json();
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
  if (transport) {
    try {
      const r = (await transport.submitCloudToken(token)) as { ok?: boolean };
      return r?.ok ?? true;
    } catch {
      return false;
    }
  }
  const res = await apiFetch("/api/v1/cloud/token", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
  });
  return res.ok;
}

/** Forget the cloud session token. */
export async function cloudLogout(): Promise<void> {
  const transport = getActiveTransport();
  if (transport) {
    await transport.cloudLogout();
    return;
  }
  await apiFetch("/api/v1/cloud/logout", { method: "POST" });
}
