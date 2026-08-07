type ToggleVariant = "heartbeat" | "preset";

interface ToggleProps {
  checked: boolean;
  onChange: () => void;
  disabled?: boolean;
  /** Renders a labeled settings row (heartbeat variant only). */
  label?: string;
  variant?: ToggleVariant;
}

/**
 * On/off switch. Two variants exist because the settings row (General) and the
 * MCP preset card use subtly different track/knob styling — kept verbatim.
 */
export function Toggle({
  checked,
  onChange,
  disabled,
  label,
  variant = "heartbeat",
}: ToggleProps) {
  if (variant === "preset") {
    return (
      <button
        type="button"
        onClick={onChange}
        disabled={disabled}
        role="switch"
        aria-checked={checked}
        className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
          disabled ? "opacity-50 cursor-not-allowed" : "cursor-pointer"
        } ${checked ? "bg-primary-500" : "bg-gray-300 dark:bg-gray-600"}`}
      >
        <span
          className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform ${
            checked ? "translate-x-[18px]" : "translate-x-[3px]"
          }`}
        />
      </button>
    );
  }
  const row = (
    <button
      onClick={onChange}
      role="switch"
      aria-checked={checked}
      className={`relative inline-flex h-5 w-9 items-center rounded-full transition ${
        checked ? "bg-primary-500" : "bg-secondary/30 dark:bg-secondary/20"
      }`}
    >
      <span
        className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition ${
          checked ? "translate-x-4.5" : "translate-x-0.5"
        }`}
      />
    </button>
  );
  if (label) {
    return (
      <div className="flex items-center justify-between">
        <label className="text-sm text-secondary">{label}</label>
        {row}
      </div>
    );
  }
  return row;
}
