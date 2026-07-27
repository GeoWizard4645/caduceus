# Writing a Caduceus extension

An extension is **one JavaScript file**. You drop it on Settings → Extensions
and it is installed. There is no folder to lay out, no separate manifest, no
build step, and nothing to `npm install`.

If you would rather not write it yourself, the Extensions tab has a prompt
starter: type one sentence about what you want, copy the prompt, paste it into
any assistant, and save the reply as a `.js` file.

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

That is the whole extension. Drop it in and it appears in the Command Center.

`input` is whatever the user typed after the command name. Return a string to
show a message, or an array of `{ title, subtitle?, action? }` to show a list.

## The header

The `@caduceus` block must be the **first** comment in the file. Caduceus parses
it *without running your code*, which is the point: the app can show what an
extension is called and everything it wants access to before a line of it has
executed.

| Key | Required | Meaning |
|---|---|---|
| `@caduceus 1` | yes | Marks the file as an extension and names the format version |
| `name` | no | Shown in the palette; defaults to the filename |
| `description` | no | One line, shown under the name |
| `author` | no | Shown in the installed list |
| `permissions` | no | Comma-separated; omit the line for none |

Only the first comment block is read. A `@caduceus` line further down the file
cannot redefine what an extension claims to be — a permission list you have to
scroll to verify is not one you can trust.

## Permissions

A closed set. Anything not on this list cannot be granted, so a typo fails at
install time instead of quietly widening what a script can reach.

| Permission | Gives you |
|---|---|
| `clipboard` | `ctx.clipboard.read()`, `ctx.clipboard.write(text)` |
| `network` | `ctx.fetch(url, init)` |
| `selection` | `ctx.selection()` — the current Finder selection |
| `notifications` | `ctx.notify(text)` |

## The `ctx` API

This is the complete list. There is no `require`, no `import`, no filesystem, no
shell, no process access, and no ambient `fetch`.

```
ctx.clipboard.read()          -> Promise<string>
ctx.clipboard.write(text)     -> Promise<void>
ctx.fetch(url, init)          -> Promise<Response>
ctx.selection()               -> Promise<string[]>
ctx.notify(text)              -> void
ctx.storage.get(key)          -> Promise<any>
ctx.storage.set(key, value)   -> Promise<void>
ctx.open(url)                 -> Promise<void>
```

`ctx.storage` is per-extension, so two extensions cannot read each other's keys.

## Installing

Drag the file onto Settings → Extensions, or put it in
`~/Library/Application Support/com.caduceus.desktop/extensions/`. No store, no
signing, no review.

## Status

**Installing works now.** Dropping a file, parsing its header, validating its
permissions, listing what is installed and removing it all ship in 2.0.0.

**Executing does not.** The sandbox that actually runs extensions is not built
yet, so an installed extension currently does nothing. The format is settled and
documented so extensions can be written against it in the meantime — and so the
format can be argued with before any code depends on it.

## Why one file

A folder with a manifest is what every other extension system does, and it is
also the reason most people never write one. The moment a contribution needs a
directory layout and two files kept in sync, it stops being something you do in
five minutes. One file with its metadata at the top cannot drift out of sync
with itself, and it fits in a single chat reply — which is what makes the prompt
starter work at all.
