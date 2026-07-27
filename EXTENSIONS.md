# Writing a Caduceus extension

An extension is **one JavaScript file**. Drop it on Settings → Extensions
and it is installed. There is no folder to lay out, no separate manifest, no
build step, and nothing to `npm install`.

The Extensions tab includes a **prompt starter** with a model of how Caduceus
works (staff, Command Center, settings, commands, AI) so an assistant can write
extensions that deeply customize the app — including paid APIs when you grant
`network` and store keys in `ctx.storage`.

## The file

```js
/**
 * @caduceus 1
 * name: Word Count
 * description: Count the words on your clipboard
 * author: you
 * permissions: clipboard
 */
export default async function (input, ctx) {
  const text = await ctx.clipboard.read();
  return `${text.trim().split(/\s+/).length} words`;
}
```

`input` is whatever the user typed after the command name. Return a string to
show a message, or an array of `{ title, subtitle?, action? }` to show a list.

## The header

The `@caduceus` block must be the **first** comment in the file. Caduceus parses
it *without running your code*.

| Key | Required | Meaning |
|---|---|---|
| `@caduceus 1` | yes | Format version |
| `name` | no | Shown in the palette |
| `description` | no | One line under the name |
| `author` | no | Shown in the installed list |
| `permissions` | no | Comma-separated; omit for none |

## Running one

Type the extension's name in the Command Center. Anything after the name is
handed to it as `input`, so `word count the quick brown fox` runs Word Count with
`the quick brown fox`.

Enter opens the extension's page, which runs it and shows what came back: a
string as text with a Copy button, an array as a list. The page keeps an input
box, so trying it again with something else does not mean retyping the query. A
row whose `action` is a function is clickable, and the function still works
because the worker that made it is still alive; if it returns a string, that
appears under the list rather than replacing it.

## Permissions

Declared in the header and enforced in **Rust on every call** — `extensions::require`
re-reads your file off disk and refuses anything the header does not claim. The
sandbox refuses first so the error is readable, but the sandbox is JavaScript
sitting beside your JavaScript; Rust is the boundary.

Editing an installed file to ask for *more* works, and is meant to — it is your
file. What does not work is anything else asking on its behalf.

**Read the permissions line before you install something.** Dropping a file
installs it; the chips in Settings → Extensions tell you what it claimed, but
they tell you afterwards. `shell` and `automation` in particular are not
capabilities in the sense the other rows are — they are "this file can do
whatever you can do", and no sandbox below is holding anything back once one of
them is granted.

| Permission | Gives you |
|---|---|
| `clipboard` | `ctx.clipboard.read()`, `ctx.clipboard.write(text)` |
| `network` | `ctx.fetch(url, init)` — any http(s) host (APIs, paid services) |
| `selection` | `ctx.selection()` — Finder selection paths |
| `notifications` | `ctx.notify(text)` |
| `shell` | `ctx.shell.run(command, input?, timeoutSecs?)` |
| `automation` | `ctx.automation.runAppleScript(script)`, `runShortcut(name, input?)` |
| `files` | `ctx.files.read(path)`, `ctx.files.write(path, content)` under `~` or app data |
| `settings` | `ctx.settings.get()`, `ctx.settings.set(fullSettings)` |
| `commands` | `ctx.commands.dispatch(paletteLine)`, `ctx.commands.runTool(toolId, input)` |
| `ai` | `ctx.ai.ask(prompt)` — primary AI backend |
| `shortcuts` | `ctx.shortcuts.run(shortcutId, query?)` |

`ctx.open(url)` and `ctx.storage` do not require a permission line (open is visible; storage is scoped per extension).

## The sandbox

An extension runs in a Web Worker: no `document`, no `window`, and — because
Tauri injects its IPC bridge into a page and not into a worker — no way to call a
Caduceus command directly. The network globals (`fetch`, `XMLHttpRequest`,
`WebSocket`, `importScripts`, `indexedDB`, …) are removed from the global scope
before your code is reached, and the app's CSP allows `connect-src 'self'` and
nothing else, so a primitive the sandbox missed still could not reach a host.
Every capability is a named Rust command instead, checked as above.

That is the shape of it, and it is worth being clear about what it does *not*
mean. The worker bounds what an extension can reach **by accident or by
cleverness**. It does not bound what it can reach **by asking**: `shell`,
`automation` and `files` are doors in that wall, and an extension that declares
them and that you install has gone through them legitimately.

A run gets 120 seconds. An extension that does not come back is terminated,
which is why an infinite loop costs a tab rather than the app.

`ctx.fetch` resolves to a Response-*like* object, not a real `Response` — the
request happened in Rust, so there is no stream left to wrap:

```
res.ok  res.status  res.statusText  res.url
res.headers.get(name)     // also res.headers["content-type"]
await res.text()          // await res.json()
```

## The `ctx` API

No `import`, `require`, or ambient `fetch`. Plain modern JavaScript run as one
script: no `export` other than the default (a named one is stripped rather than
refused, so a file that has one still works).

```
ctx.clipboard.read() / write(text)
ctx.fetch(url, init)
ctx.selection()
ctx.notify(text)
ctx.storage.get(key) / set(key, value)   — up to 2 MB per extension
ctx.open(url)

ctx.shell.run(command, input?, timeoutSecs?)
ctx.automation.runAppleScript(script)
ctx.automation.runShortcut(name, input?)
ctx.files.read(path) / write(path, content)
ctx.settings.get() / set(settings)
ctx.commands.dispatch(input)
ctx.commands.runTool(toolId, input)      — sha256, json_format, uuid, …
ctx.ai.ask(prompt)
ctx.shortcuts.run(shortcutId, query?)
```

## Limits (relaxed for real extensions)

| Limit | Value |
|---|---|
| Source file size | 2 MB |
| Run timeout | 120 s |
| `ctx.fetch` body | 25 MB |
| List rows | 500 |
| Per-extension storage | 2 MB |

## Installing

Drag onto Settings → Extensions, or put the file in  
`~/Library/Application Support/com.caduceus.desktop/extensions/`.

## Customization examples

- **Theme / staff** — `settings` permission: read settings, change `appearance`, write back with `ctx.settings.set`.
- **Paid API** — `network` + `storage`: store API key in `ctx.storage`, call `ctx.fetch`.
- **AI workflow** — `ai` or `commands.dispatch("/ your question")`.
- **macOS apps** — `automation` or `shell`.

See `src/shared/extensionAppModel.ts` for the full architecture blurb copied into the AI prompt.

## Status

**Installing and running both work**, as of 3.1.2. The one-file format has not
changed since it was published in 2.0.0 — which was the point of publishing it
before the runtime existed.
