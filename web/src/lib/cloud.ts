// Syscity Cloud session helpers (same-origin with the gateway).

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
  const res = await fetch("/api/v1/status");
  if (!res.ok) throw new Error(`status: HTTP ${res.status}`);
  const body = await res.json();
  const cloud = body.cloud as CloudStatus | null | undefined;
  if (!cloud) return { enabled: false, logged_in: false, user: null };
  return cloud;
}

/** Plan + credit balance (+ low-credit/overdraft flags). */
export async function cloudSubscription(): Promise<CloudSubscription> {
  const res = await fetch("/api/v1/cloud/subscription");
  if (!res.ok) throw new Error(`subscription: HTTP ${res.status}`);
  return res.json();
}

/** Credit usage for the last `days` (default 30). */
export async function cloudUsage(days = 30): Promise<CloudUsage> {
  const res = await fetch(`/api/v1/cloud/usage?days=${days}`);
  if (!res.ok) throw new Error(`usage: HTTP ${res.status}`);
  return res.json();
}

/** The cloud OAuth login URL (engine route that 302s to the cloud). */
export function cloudLoginUrl(provider = "github"): string {
  return `/api/v1/cloud/login?provider=${provider}`;
}

/** Redirect the current tab to the cloud OAuth login (welcome-page flow). */
export function cloudLogin(provider = "github") {
  window.location.href = cloudLoginUrl(provider);
}

/** Persist a session token returned by the cloud OAuth callback. */
export async function cloudSubmitToken(token: string): Promise<boolean> {
  const res = await fetch("/api/v1/cloud/token", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
  });
  return res.ok;
}

/** Forget the cloud session token. */
export async function cloudLogout(): Promise<void> {
  await fetch("/api/v1/cloud/logout", { method: "POST" });
}
