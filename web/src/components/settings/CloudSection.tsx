import { useEffect, useState } from "react";
import { Section } from "@/components/ui/Section";
import {
  cloudLogin,
  cloudLogout,
  cloudStatus,
  type CloudStatus,
} from "@/lib/cloud";

/** Syscity Cloud login status + actions. Hidden when cloud is disabled. */
export function CloudSection() {
  const [status, setStatus] = useState<CloudStatus | null>(null);

  useEffect(() => {
    cloudStatus().then(setStatus).catch(() => setStatus(null));
  }, []);

  if (!status?.enabled) return null;

  const user = status.user;
  const display = user?.name ?? user?.email ?? user?.id ?? "Cloud user";

  return (
    <Section title="Syscity Cloud">
      <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-1 sm:gap-0 px-3 py-2 rounded-lg bg-card">
        <div className="min-w-0">
          <p className="text-sm text-primary">
            {status.logged_in ? "Signed in" : "Not signed in"}
          </p>
          {status.logged_in && (
            <p className="truncate text-xs text-secondary">{display}</p>
          )}
        </div>
        {status.logged_in ? (
          <button
            type="button"
            onClick={() => {
              void cloudLogout();
              setStatus({ ...status, logged_in: false, user: null });
            }}
            className="rounded-lg border border-subtle px-3 py-1.5 text-sm font-semibold text-primary transition hover:border-primary-500 hover:text-primary-500"
          >
            Sign out
          </button>
        ) : (
          <button
            type="button"
            onClick={() => cloudLogin("github")}
            className="rounded-lg border border-subtle px-3 py-1.5 text-sm font-semibold text-primary transition hover:border-primary-500 hover:text-primary-500"
          >
            Sign in
          </button>
        )}
      </div>
      {status.logged_in && (
        <p className="px-3 text-xs text-tertiary">
          Cloud models are available in the model picker.
        </p>
      )}
    </Section>
  );
}
