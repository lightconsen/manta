// Syscity Cloud session helpers (same-origin with the gateway).

export interface CloudStatus {
  enabled: boolean;
  logged_in: boolean;
  user: { id?: string; name?: string; email?: string | null } | null;
}

export async function cloudStatus(): Promise<CloudStatus> {
  const res = await fetch("/api/v1/cloud/status");
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
