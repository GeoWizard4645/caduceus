# Cutting a Caduceus release

Caduceus is **macOS-only**. Each public release is one **universal** `.dmg` (Apple Silicon + Intel) attached to a GitHub release. The website installer (`curl … install.sh`) downloads whatever `/releases/latest` points at — you do **not** redeploy the Worker for a new app version unless `install.sh` itself changed.

## The short version

```bash
npm run release -- patch -m "fix a hotkey that could never fire"
```

That is the whole thing. [`scripts/release.sh`](./scripts/release.sh) does every step below in order: bumps the three version files, runs the gates and the Rust tests, builds both architectures, `lipo`s them into one universal app, signs it, packs the DMG, commits, tags, pushes, creates the release with `--latest`, updates the Homebrew tap, and checks that `/releases/latest` now resolves to it.

| | |
|---|---|
| `patch` \| `minor` \| `major` \| `2.4.0` | what to bump to |
| `-m "…"` | the summary. Becomes the commit subject and the top of the release notes. Prompted for if you leave it out |
| `-a`, `--all` | commit everything in the tree, not just the version bump. Without it, a dirty tree stops the release |
| `-n`, `--dry-run` | print every step and change nothing. Worth doing once |
| `--draft` | publish as a draft, so it does not become `/releases/latest` yet |
| `--notes-file F` | use a file for the notes instead of the commit log |
| `--skip-tests` | skip the gates. For re-runs after a failure, not for releases |
| `--skip-cask` | leave the Homebrew tap alone |

It refuses to start unless you are on `main`, up to date with `origin`, logged into `gh`, and the tag is free — and it does not push anything until the DMG exists on disk, so a build that dies half-way costs you time rather than leaving a tag pointing at nothing.

The rest of this document is what the script does, for the times you need to do it by hand or work out why it stopped.

## Prerequisites

- macOS with **Xcode Command Line Tools** (`xcode-select --install`)
- **Rust** ([rustup](https://rustup.rs)), **Node 20+**, repo dependencies (`npm install`)
- **Intel cross-target** (one-time): `rustup target add x86_64-apple-darwin`
- **[GitHub CLI](https://cli.github.com/)** logged in: `gh auth login`
- Write access to [GeoWizard4645/caduceus](https://github.com/GeoWizard4645/caduceus)

## 1. Bump the version

Keep these three in sync (semver, e.g. `1.0.3`):

| File | Field |
|------|--------|
| `package.json` | `"version"` |
| `src-tauri/tauri.conf.json` | `"version"` |
| `src-tauri/Cargo.toml` | `version = "…"` under `[package]` |

Commit and push to `main` (or merge your PR) **before** tagging.

## 2. Sanity-check the tree

From the repo root:

```bash
npm run typecheck
npm run test:rust
```

Optional local smoke test:

```bash
npm run tauri -- build --bundles app
open src-tauri/target/release/bundle/macos/Caduceus.app
```

## 3. Build the universal `.dmg`

Stable Rust on Apple Silicon does **not** ship a `universal-apple-darwin` target, so we build **two** app bundles and **lipo** the main executable, then pack a DMG with `hdiutil`.

### 3a. Apple Silicon app (native)

```bash
npm run tauri -- build --bundles app
```

Output: `src-tauri/target/release/bundle/macos/Caduceus.app`

### 3b. Intel app (cross-compile)

```bash
npm run tauri -- build --target x86_64-apple-darwin --bundles app
```

Output: `src-tauri/target/x86_64-apple-darwin/release/bundle/macos/Caduceus.app`

### 3c. Merge into one universal app + DMG

Run from the repo root (adjust `VERSION` if needed):

```bash
VERSION=2.0.0   # match package.json

ROOT="src-tauri/target"
APP_A="$ROOT/release/bundle/macos/Caduceus.app"
APP_X="$ROOT/x86_64-apple-darwin/release/bundle/macos/Caduceus.app"
STAGE="$ROOT/release/bundle/macos/Caduceus_universal_staging"
DMG="src-tauri/target/release/bundle/dmg/Caduceus_${VERSION}_universal.dmg"

rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -R "$APP_A" "$STAGE/Caduceus.app"
UNI="$STAGE/Caduceus.app"

lipo -create \
  "$APP_A/Contents/MacOS/Caduceus" \
  "$APP_X/Contents/MacOS/Caduceus" \
  -output "$UNI/Contents/MacOS/Caduceus"

codesign --force --sign - --identifier com.caduceus.desktop "$UNI"

mkdir -p "$(dirname "$DMG")"
rm -f "$DMG"
hdiutil create -volname "Caduceus" -srcfolder "$STAGE" -ov -format UDZO "$DMG"

file "$UNI/Contents/MacOS/Caduceus"   # should say "universal binary … arm64 … x86_64"
ls -lh "$DMG"
```

**Asset name:** `Caduceus_<version>_universal.dmg` (e.g. `Caduceus_1.0.2_universal.dmg`). The install script prefers filenames containing `universal`.

**Note:** the Swift helpers under `Contents/Resources/bin/` — `caduceus-stt`, `caduceus-stt-live` and `caduceus-native` — are built for the host arch only; only the main `Caduceus` binary is fat. That matches how prior releases were assembled.

`caduceus-native` (Vision OCR and CoreAudio device switching) is compiled by `build.rs` like the speech helpers, and is signed with its own identifier because it needs no privacy grant of its own. If `swiftc` is unavailable at build time, the build still succeeds and those two features report that the helper is missing rather than failing silently.

### Shortcut: single-arch DMG (not for public release)

`npm run bundle` produces `Caduceus_<version>_aarch64.dmg` on Apple Silicon. Use that for quick local tests only — public releases should ship the universal DMG above.

If `npm run bundle` fails once on the DMG step, retry `npm run tauri -- build --bundles dmg`; the `.app` is often already built.

## 4. Create the GitHub release

Tag format: **`v` + semver** (e.g. `v2.0.0`), pointing at the commit that contains the version bump.

```bash
VERSION=2.0.0
TAG="v${VERSION}"
DMG="src-tauri/target/release/bundle/dmg/Caduceus_${VERSION}_universal.dmg"

gh release create "$TAG" "$DMG" \
  --repo GeoWizard4645/caduceus \
  --title "$TAG" \
  --latest \
  --notes "$(cat <<EOF
Short summary of what changed.

## Install

\`\`\`bash
curl -fsSL https://vivaanshahani.com/caduceus/install.sh | bash
\`\`\`

## Issues

https://github.com/GeoWizard4645/caduceus/issues
EOF
)"
```

- **`--latest`** makes this build show up as `/releases/latest` (what the installer uses).
- Do **not** mark stable releases as pre-release unless you intend betas to stay on the fallback API path.

Release page: `https://github.com/GeoWizard4645/caduceus/releases/tag/v<version>`

## 5. Update the Homebrew tap

```bash
scripts/publish-cask.sh 2.4.0
```

`npm run release` does this automatically, after the GitHub release exists — the cask carries a checksum of the attached asset, so it cannot be published first.

The script rewrites the version and `sha256` in [`homebrew/caduceus.rb`](./homebrew/caduceus.rb), which is the source of truth, and copies it to `Casks/caduceus.rb` in **[GeoWizard4645/homebrew-caduceus](https://github.com/GeoWizard4645/homebrew-caduceus)** — a separate one-file repository, because Homebrew resolves `brew tap owner/name` to `github.com/owner/homebrew-name` and there is no way around that naming. The first run offers to create it.

Called with no DMG path it downloads the asset from the release and checksums that, which is the more useful check of the two: it verifies the bytes a user will actually receive.

```bash
brew install --cask geowizard4645/caduceus/caduceus   # what the tap gives people
brew upgrade --cask caduceus
brew uninstall --zap --cask caduceus                  # app plus everything it stored
```

**Why a tap and not `homebrew/homebrew-cask`.** The official repository has a notability bar Caduceus does not clear yet and a review queue measured in days; a tap ships the moment the release does. Moving to homebrew-cask later would not change the command anyone has already run — `brew install --cask caduceus` would simply start resolving to the official one.

## 6. Verify

```bash
# Latest release metadata
curl -fsSL "https://api.github.com/repos/GeoWizard4645/caduceus/releases/latest" \
  | grep -E '"tag_name"|Caduceus_.*\.dmg'

# End-to-end install (optional)
curl -fsSL https://vivaanshahani.com/caduceus/install.sh | bash
```

Confirm the log line shows **`Caduceus_<version>_universal.dmg`**, then open the app from `/Applications/Caduceus.app`.

## Checklist

Only for a hand-rolled release — `npm run release` enforces all of it.

- [ ] Version bumped in `package.json`, `tauri.conf.json`, `Cargo.toml`
- [ ] Changes committed on `main`
- [ ] `npm run typecheck` (and tests if you touched Rust)
- [ ] Universal `.dmg` built and `file` shows arm64 + x86_64 on main binary
- [ ] `gh release create` with `Caduceus_<version>_universal.dmg` and `--latest`
- [ ] Homebrew tap updated (`scripts/publish-cask.sh <version>`)
- [ ] Installer or API check confirms `/releases/latest` is the new tag

## Related docs

- [README.md](./README.md) — development build and local install
- [website/install.sh](./website/install.sh) — what users run; redeploy only when this script changes (Cloudflare Worker / site)
