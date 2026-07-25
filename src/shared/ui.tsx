/**
 * Shared UI primitives.
 *
 * Small, unopinionated, and deliberately not a component library: each one is a
 * styled element with the props it actually needs. The point is that Settings
 * and the Command Center look like the same product without either importing
 * the other's internals.
 */

import type { ReactNode } from "react";
import { useEffect, useId, useRef, useState } from "react";

export function cx(...values: (string | false | null | undefined)[]): string {
  return values.filter(Boolean).join(" ");
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

type ButtonTone = "primary" | "default" | "ghost" | "danger";

export function Button({
  children,
  onClick,
  tone = "default",
  disabled,
  type = "button",
  size = "md",
  title,
  className,
}: {
  children: ReactNode;
  onClick?: () => void;
  tone?: ButtonTone;
  disabled?: boolean;
  type?: "button" | "submit";
  size?: "sm" | "md";
  title?: string;
  className?: string;
}) {
  const tones: Record<ButtonTone, string> = {
    primary:
      "bg-accent text-accent-ink hover:brightness-110 active:brightness-95 shadow-sm border-transparent",
    default:
      "bg-raised text-ink hover:bg-overlay border-line-strong/60 shadow-inner-hair",
    ghost: "bg-transparent text-ink-soft hover:bg-raised hover:text-ink border-transparent",
    danger:
      "bg-transparent text-danger hover:bg-danger/10 border-danger/30",
  };

  return (
    <button
      type={type}
      title={title}
      onClick={onClick}
      disabled={disabled}
      className={cx(
        "no-drag inline-flex shrink-0 items-center justify-center gap-1.5 rounded-lg border font-medium",
        "transition-[background-color,filter,border-color] duration-150 ease-cad",
        "disabled:cursor-not-allowed disabled:opacity-40",
        size === "sm" ? "h-7 px-2.5 text-2xs" : "h-9 px-3.5 text-[13px]",
        tones[tone],
        className,
      )}
    >
      {children}
    </button>
  );
}

export function IconButton({
  children,
  onClick,
  label,
  tone = "ghost",
  disabled,
}: {
  children: ReactNode;
  onClick?: () => void;
  label: string;
  tone?: ButtonTone;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      disabled={disabled}
      className={cx(
        "no-drag inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-[13px]",
        "transition-colors duration-150 disabled:opacity-40",
        tone === "danger"
          ? "text-ink-mute hover:bg-danger/12 hover:text-danger"
          : "text-ink-mute hover:bg-raised hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Form fields
// ---------------------------------------------------------------------------

export function Field({
  label,
  hint,
  children,
  error,
  wide,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
  error?: string | null;
  wide?: boolean;
}) {
  return (
    <label className={cx("block", wide && "col-span-2")}>
      <span className="mb-1.5 block text-[13px] font-medium text-ink-soft">{label}</span>
      {children}
      {error ? (
        <span className="mt-1.5 block text-2xs text-danger">{error}</span>
      ) : hint ? (
        <span className="mt-1.5 block text-2xs leading-relaxed text-ink-faint">{hint}</span>
      ) : null}
    </label>
  );
}

const inputClass =
  "w-full rounded-lg border border-line-strong/60 bg-base/60 px-3 py-2 text-[13px] text-ink " +
  "placeholder:text-ink-faint transition-[border-color,box-shadow] duration-150 " +
  "focus:border-accent/70 focus:shadow-[0_0_0_3px_rgb(var(--c-accent)/0.18)] focus:outline-none " +
  "disabled:opacity-50";

export function TextInput({
  value,
  onChange,
  placeholder,
  type = "text",
  disabled,
  mono,
  onBlur,
  autoFocus,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: "text" | "password" | "url";
  disabled?: boolean;
  mono?: boolean;
  onBlur?: () => void;
  autoFocus?: boolean;
}) {
  return (
    <input
      type={type}
      value={value}
      disabled={disabled}
      placeholder={placeholder}
      autoFocus={autoFocus}
      spellCheck={false}
      autoComplete="off"
      autoCorrect="off"
      onChange={(e) => onChange(e.target.value)}
      onBlur={onBlur}
      className={cx(inputClass, mono && "font-mono text-2xs")}
    />
  );
}

export function NumberInput({
  value,
  onChange,
  min,
  max,
  step = 1,
  suffix,
}: {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  suffix?: string;
}) {
  return (
    <div className="relative">
      <input
        type="number"
        value={Number.isFinite(value) ? value : 0}
        min={min}
        max={max}
        step={step}
        onChange={(e) => {
          const next = Number(e.target.value);
          if (!Number.isNaN(next)) onChange(next);
        }}
        className={cx(inputClass, suffix && "pr-14")}
      />
      {suffix && (
        <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-2xs text-ink-faint">
          {suffix}
        </span>
      )}
    </div>
  );
}

export function TextArea({
  value,
  onChange,
  placeholder,
  rows = 3,
  mono,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  rows?: number;
  mono?: boolean;
}) {
  return (
    <textarea
      value={value}
      rows={rows}
      placeholder={placeholder}
      spellCheck={false}
      onChange={(e) => onChange(e.target.value)}
      className={cx(inputClass, "resize-y leading-relaxed", mono && "font-mono text-2xs")}
    />
  );
}

export function Select<T extends string>({
  value,
  onChange,
  options,
  disabled,
}: {
  value: T;
  onChange: (value: T) => void;
  options: { value: T; label: string; disabled?: boolean }[];
  disabled?: boolean;
}) {
  return (
    <div className="relative">
      <select
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value as T)}
        className={cx(inputClass, "cursor-pointer appearance-none pr-9")}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value} disabled={option.disabled}>
            {option.label}
          </option>
        ))}
      </select>
      <svg
        className="pointer-events-none absolute right-3 top-1/2 h-3 w-3 -translate-y-1/2 text-ink-faint"
        viewBox="0 0 12 12"
        fill="none"
        aria-hidden="true"
      >
        <path d="M3 4.5 6 7.5 9 4.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      </svg>
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  hint?: ReactNode;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div className="flex items-start gap-3 py-1">
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => onChange(!checked)}
        className={cx(
          "no-drag relative mt-0.5 h-[22px] w-[38px] shrink-0 rounded-full border transition-colors duration-200 ease-cad",
          "disabled:cursor-not-allowed disabled:opacity-40",
          checked ? "border-accent/50 bg-accent" : "border-line-strong bg-raised",
        )}
      >
        {/* Positioned with `left`, not `translate-x`: an absolutely positioned
            element with no `left` falls back to its static position, which a
            <button> centres, so a transform-only knob lands 18px off. */}
        <span
          className={cx(
            "absolute top-1/2 h-[16px] w-[16px] -translate-y-1/2 rounded-full bg-white shadow-sm",
            "transition-[left] duration-200 ease-cad",
            checked ? "left-[19px]" : "left-[3px]",
          )}
        />
      </button>
      <label htmlFor={id} className="min-w-0 cursor-pointer select-none">
        <span className="block text-[13px] font-medium text-ink">{label}</span>
        {hint && <span className="mt-0.5 block text-2xs leading-relaxed text-ink-faint">{hint}</span>}
      </label>
    </div>
  );
}

/** Captures a real key combination instead of asking the user to type "CmdOrCtrl+K". */
export function HotkeyInput({
  value,
  onChange,
  placeholder = "Click, then press a key combination",
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
}) {
  const [capturing, setCapturing] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!capturing) return;

    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        setCapturing(false);
        return;
      }
      // Modifiers alone are not a binding; wait for a real key.
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

      const parts: string[] = [];
      // `CommandOrControl` maps to ⌘ on macOS and Ctrl elsewhere, which is what
      // a user pressing either one means.
      if (e.metaKey) parts.push("CommandOrControl");
      else if (e.ctrlKey) parts.push("Control");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");

      const key =
        e.code.startsWith("Key") ? e.code.slice(3)
        : e.code.startsWith("Digit") ? e.code.slice(5)
        : e.code.startsWith("Numpad") ? `Num${e.code.slice(6)}`
        : e.key === " " ? "Space"
        : e.key.length === 1 ? e.key.toUpperCase()
        : e.key;

      parts.push(key);
      onChange(parts.join("+"));
      setCapturing(false);
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing, onChange]);

  return (
    <div className="flex items-center gap-2">
      <button
        ref={ref}
        type="button"
        onClick={() => setCapturing((c) => !c)}
        onBlur={() => setCapturing(false)}
        className={cx(
          inputClass,
          "no-drag flex h-[38px] items-center justify-between text-left font-mono",
          capturing && "border-accent/70 shadow-[0_0_0_3px_rgb(var(--c-accent)/0.18)]",
        )}
      >
        <span className={value ? "text-ink" : "text-ink-faint"}>
          {capturing ? "Press keys… (Esc to cancel)" : value || placeholder}
        </span>
      </button>
      {value && (
        <IconButton label="Clear shortcut" onClick={() => onChange("")}>
          ×
        </IconButton>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

export function Section({
  title,
  description,
  children,
  actions,
}: {
  title: string;
  description?: ReactNode;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section className="mb-8">
      <div className="mb-3 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-[15px] font-semibold tracking-[-0.01em] text-ink">{title}</h2>
          {description && (
            <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">{description}</p>
          )}
        </div>
        {actions && <div className="row shrink-0">{actions}</div>}
      </div>
      <div className="rounded-cad border border-line bg-surface/50 p-5">{children}</div>
    </section>
  );
}

export function Callout({
  tone = "info",
  children,
  title,
}: {
  tone?: "info" | "warn" | "danger" | "positive";
  children: ReactNode;
  title?: string;
}) {
  const tones = {
    info: "border-accent/25 bg-accent/[0.07] text-ink-soft",
    warn: "border-caution/30 bg-caution/[0.08] text-ink-soft",
    danger: "border-danger/30 bg-danger/[0.08] text-ink-soft",
    positive: "border-positive/30 bg-positive/[0.08] text-ink-soft",
  };
  const marks = { info: "i", warn: "!", danger: "!", positive: "✓" };

  return (
    <div className={cx("flex gap-3 rounded-lg border px-3.5 py-3 text-[13px] leading-relaxed", tones[tone])}>
      <span
        aria-hidden="true"
        className={cx(
          "mt-px flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-full text-2xs font-bold",
          tone === "info" && "bg-accent/20 text-accent",
          tone === "warn" && "bg-caution/20 text-caution",
          tone === "danger" && "bg-danger/20 text-danger",
          tone === "positive" && "bg-positive/20 text-positive",
        )}
      >
        {marks[tone]}
      </span>
      <div className="min-w-0">
        {title && <p className="mb-1 font-semibold text-ink">{title}</p>}
        <div className="[&_a]:text-accent [&_a]:underline [&_a]:underline-offset-2">{children}</div>
      </div>
    </div>
  );
}

export function EmptyState({ title, hint, icon }: { title: string; hint?: ReactNode; icon?: string }) {
  return (
    <div className="flex flex-col items-center justify-center px-6 py-12 text-center">
      {icon && (
        <div className="mb-3 flex h-11 w-11 items-center justify-center rounded-full border border-line bg-raised text-lg text-ink-faint">
          {icon}
        </div>
      )}
      <p className="text-[13px] font-medium text-ink-soft">{title}</p>
      {hint && <p className="mt-1.5 max-w-sm text-2xs leading-relaxed text-ink-faint">{hint}</p>}
    </div>
  );
}

export function Spinner({ className }: { className?: string }) {
  return (
    <svg
      className={cx("h-3.5 w-3.5 animate-spin text-current", className)}
      viewBox="0 0 16 16"
      fill="none"
      aria-hidden="true"
    >
      <circle cx="8" cy="8" r="6.5" stroke="currentColor" strokeOpacity="0.22" strokeWidth="2" />
      <path d="M14.5 8A6.5 6.5 0 0 0 8 1.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}
