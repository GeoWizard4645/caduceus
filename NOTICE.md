# Licence and attribution

Caduceus is **GPL-3.0-or-later**. Copyright © 2026 Vivaan Shahani and Caduceus
contributors.

## Why it changed

Caduceus was MIT up to and including v2.3.1. It moved to GPL-3.0 in **v3.0.0**,
deliberately, so that its feature set could be developed alongside — and where
useful, informed by — two GPL-3.0 projects that solve problems Caduceus now also
solves:

- **[MacParakeet](https://github.com/moona3k/macparakeet)** — GPL-3.0,
  copyright © 2026 Daniel Moon. On-device dictation, meeting recording with a
  live transcript, and a floating recording indicator.
- **[Clean-Me](https://github.com/Kevin-De-Koninck/Clean-Me)** — a macOS system
  cleaner: caches, logs, and the leftovers of removed applications.

Every release before v3.0.0 remains available under the MIT licence it was
published with. An MIT grant cannot be withdrawn retroactively, and this does
not attempt to.

## What was actually taken

**No source code was copied from either project.** Caduceus is Rust, TypeScript
and a handful of small Swift command-line helpers; MacParakeet is a Swift 6 /
SwiftUI application and Clean-Me is a Swift/AppKit one. There is no file in this
repository that originated in either.

What they contributed is *design*: the shape of a recording HUD that stays out of
your way, the decision to keep a live transcript visible while recording, and the
category-by-category approach to reclaiming disk space. Those are ideas, not
code, and ideas are not what a licence covers.

The licence changed anyway, for two reasons: so that borrowing directly is
available in future without a second round of legal thinking, and because it is
the honest thing to do when a project's feature set is this visibly informed by
someone else's work.

If you maintain either project and feel the attribution here is wrong or
insufficient, open an issue — it will be fixed.

## The practical consequences

- You may use, modify and redistribute Caduceus.
- If you distribute a modified version, you must publish your changes under
  GPL-3.0 as well.
- The Mac App Store is effectively closed to GPL-3.0 software. Caduceus is
  distributed through its own installer, a Homebrew tap and GitHub releases, so
  this costs nothing today.

## Other components

Caduceus links a large number of Rust and JavaScript dependencies under
permissive licences (MIT, Apache-2.0, BSD). `cargo tree` and `npm ls` enumerate
them. Nothing GPL is currently linked into the binary; the licence change above
is about the project's own terms, not an obligation inherited from a dependency.

Apple frameworks used through their public APIs — Speech, AVFoundation,
ScreenCaptureKit, Vision, AppKit, CoreAudio — are governed by the macOS SDK
licence and are not redistributed.
