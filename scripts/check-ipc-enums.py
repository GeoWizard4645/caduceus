#!/usr/bin/env python3
"""Check that every TypeScript string union matches its Rust enum.

Several of Caduceus's IPC commands take a closed enum rather than a free string:
`run_tool` takes a `ToolId`, `system_action` a `SystemAction`, `window_action` a
`Verb`. That is what keeps the IPC surface narrow — the webview can name a tool
that exists and nothing else.

The cost is about ninety string constants written down twice, in Rust and in
TypeScript, with serde's `snake_case` renaming in between. Neither compiler can
see the other side, so a typo or a renamed variant produces a command that
type-checks, builds, ships, and then fails the first time somebody runs it.

This closes that gap. Run with `npm run check:ipc`; it is part of `npm run build`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# (TypeScript type, Rust file, Rust enum)
CHECKS = [
    ("ToolId", "src-tauri/src/tools/dev.rs", "ToolId"),
    ("SystemAction", "src-tauri/src/tools/system.rs", "SystemAction"),
    ("MediaAction", "src-tauri/src/tools/media.rs", "MediaAction"),
    ("WindowVerb", "src-tauri/src/window/manage.rs", "Verb"),
]


def to_snake_case(name: str) -> str:
    """`TopLeftQuarter` -> `top_left_quarter`, matching serde's rename_all."""
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def rust_variants(relative_path: str, enum: str) -> list[str]:
    source = (ROOT / relative_path).read_text()
    match = re.search(rf"enum\s+{enum}\s*\{{(.*?)\n\}}", source, re.S)
    if not match:
        sys.exit(f"could not find `enum {enum}` in {relative_path}")

    body = re.sub(r"//.*", "", match.group(1))
    body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
    # Variants are bare identifiers followed by a comma; anything with a payload
    # would not round-trip as a plain string and is deliberately not matched.
    return [to_snake_case(name) for name in re.findall(r"^\s*([A-Z]\w*)\s*,", body, re.M)]


def ts_union(type_name: str) -> list[str]:
    source = (ROOT / "src/shared/types.ts").read_text()
    match = re.search(rf"export type {type_name}\s*=\s*(.*?);", source, re.S)
    if not match:
        sys.exit(f"could not find `export type {type_name}` in src/shared/types.ts")
    return re.findall(r'"([a-z0-9_]+)"', match.group(1))


def main() -> int:
    failures = 0

    for ts_name, rust_path, rust_enum in CHECKS:
        rust = rust_variants(rust_path, rust_enum)
        typescript = ts_union(ts_name)

        missing_from_ts = [v for v in rust if v not in typescript]
        missing_from_rust = [v for v in typescript if v not in rust]

        if missing_from_ts or missing_from_rust:
            failures += 1
            print(f"MISMATCH {ts_name} ({rust_enum} in {rust_path})")
            if missing_from_ts:
                print(f"  in Rust but not in TypeScript: {missing_from_ts}")
            if missing_from_rust:
                print(f"  in TypeScript but not in Rust: {missing_from_rust}")
        else:
            print(f"ok  {ts_name:<14} {len(rust)} variants")

    # Every tool the command registry wires up must be a real ToolId.
    commands = (ROOT / "src/shared/commands.ts").read_text()
    block = commands[commands.index("const TOOL_SPECS") : commands.index("const TOOL_COMMANDS")]
    referenced = set(re.findall(r'id: "([a-z0-9_]+)"', block))
    unknown = referenced - set(ts_union("ToolId"))

    if unknown:
        failures += 1
        print(f"MISMATCH commands.ts references tools that do not exist: {sorted(unknown)}")
    else:
        print(f"ok  TOOL_SPECS     {len(referenced)} ids, all valid")

    if failures:
        print(f"\n{failures} mismatch(es). These would fail at runtime, not at build time.")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
