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

export async function cloudStatus(): Promise<CloudStatus> {
  const res = await fetch("/api/v1/cloud/status");
  return res.json();
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

/** Redirect to the cloud OAuth login for a provider. */
export function cloudLogin(provider = "github") {
  window.location.href = `/api/v1/cloud/login?provider=${provider}`;
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
