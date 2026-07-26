# Writing a Caduceus extension

> **Status: design, not yet implemented.** This document is the agreed shape of
> the extension system so that work can start against a fixed target. Nothing
> below loads in Caduceus 1.1.0 yet. It is checked in rather than kept in a
> ticket because the format is the part worth arguing about *before* anything
> depends on it.

An extension adds commands to the Command Center. It is a folder with a
manifest and a script, and it does not need to be compiled.

```
my-extension/
  caduceus.json      # manifest
  main.js            # the commands
  icon.png           # optional, 128×128
```

## The manifest

```json
{
  "id": "com.example.my-extension",
  "name": "My Extension",
  "version": "1.0.0",
  "author": "you",
  "description": "One line, shown in the extension list.",
  "license": "MIT",
  "commands": [
    {
      "id": "greet",
      "title": "Greet Someone",
      "subtitle": "Says hello",
      "mode": "view",
      "arguments": [{ "name": "who", "placeholder": "name", "required": true }]
    }
  ],
  "permissions": ["clipboard", "network"]
}
```

`mode` is either `view` (renders a list of results) or `action` (runs and shows
a toast). `permissions` is a closed set — an extension that does not ask for
`network` is not given it.

## The script

```js
export async function greet({ who }, ctx) {
  await ctx.clipboard.write(`Hello, ${who}`);
  return ctx.toast(`Copied a greeting for ${who}`);
}
```

`ctx` is the only way out of the sandbox. There is no `require`, no `fs`, no
`child_process`, and no ambient `fetch` — a command that wants the network asks
for it in the manifest and receives `ctx.fetch`, which is restricted to the
hosts the manifest declares.

| API | Needs permission | Does |
|---|---|---|
| `ctx.toast(msg)` | — | Shows a message in the palette |
| `ctx.list(items)` | — | Returns rows to display |
| `ctx.clipboard.read()` / `.write(text)` | `clipboard` | The system clipboard |
| `ctx.fetch(url, init)` | `network` | HTTP to declared hosts only |
| `ctx.storage.get/set(key, value)` | — | Per-extension key/value store |
| `ctx.open(url)` | — | Opens a URL in the default browser |
| `ctx.selection()` | `selection` | Current Finder selection |

## Installing

Drop the folder in `~/Library/Application Support/com.caduceus.desktop/extensions/`
and it appears in the Command Center. There is no store, no signing, and no
review — but also no sandbox escape hatch, which is the trade.

## Why this shape

**No build step.** An extension you can write in one file and drop in a folder
is one people actually write. A toolchain is a reason not to start.

**Permissions in the manifest, not at runtime.** You can read what an extension
is allowed to do before you run it, from a file, without executing anything.

**No ambient capability.** The sandbox denies by default. Everything an
extension can reach arrives through `ctx`, so the list above is the complete
answer to "what can this thing do to my machine".

## Contributing one

Open a PR against `extensions/` in this repo and it ships with Caduceus, or host
the folder anywhere and let people download it. Both are first-class; there is
no blessed channel.
