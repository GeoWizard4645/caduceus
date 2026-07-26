/**
 * Render a Tauri accelerator the way a Mac user reads it.
 *
 * The walkthrough and the Help tab both have to say "press <this>", and the key
 * is not fixed: if the configured one is taken by another app, Caduceus rebinds
 * to a free fallback at startup. Hardcoding "Control + Space" in prose would be
 * wrong for exactly the users who most need the instruction to be right.
 */

const MAC_SYMBOLS: Record<string, string> = {
  commandorcontrol: "\u2318",
  command: "\u2318",
  cmd: "\u2318",
  super: "\u2318",
  meta: "\u2318",
  control: "\u2303",
  ctrl: "\u2303",
  alt: "\u2325",
  option: "\u2325",
  shift: "\u21e7",
};

export function hotkeyLabel(accelerator: string, platform = "macos"): string {
  const trimmed = accelerator.trim();
  if (!trimmed) return "";

  const parts = trimmed.split("+").map((p) => p.trim()).filter(Boolean);
  if (platform !== "macos") return parts.join(" + ");

  return parts
    .map((part) => {
      const symbol = MAC_SYMBOLS[part.toLowerCase()];
      if (symbol) return symbol;
      return part.length === 1 ? part.toUpperCase() : part;
    })
    .join("");
}

/**
 * Where the Command Center hotkey currently lives — the dedicated accelerator,
 * or the function-key row bound to it, or nothing.
 */
export function commandCenterKey(settings: {
  general: { commandCenterHotkey: string; functionKeys: { key: string; action: string }[] };
}): string {
  const direct = settings.general.commandCenterHotkey.trim();
  if (direct) return direct;
  return (
    settings.general.functionKeys.find((b) => b.action === "command_center")?.key ?? ""
  );
}

/** Same, for showing/hiding the staff. */
export function toggleStaffKey(settings: {
  general: { toggleOrbHotkey: string; functionKeys: { key: string; action: string }[] };
}): string {
  const direct = settings.general.toggleOrbHotkey.trim();
  if (direct) return direct;
  return settings.general.functionKeys.find((b) => b.action === "toggle_staff")?.key ?? "";
}
