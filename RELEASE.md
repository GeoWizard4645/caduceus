# Cutting a Caduceus release

Caduceus is **macOS-only**. Each public release is one **universal** `.dmg` (Apple Silicon + Intel) attached to a GitHub release. The website installer (`curl … install.sh`) downloads whatever `/releases/latest` points at — you do **not** redeploy the Worker for a new app version unless `install.sh` itself changed.

## The short version

```bash
npm run release -- patch -m "fix a hotkey that could never fire"
```

That is the whole thing. [`scripts/release.sh`](./scripts/release.sh) does every step below in order: bumps the four version files, runs the gates and the Rust tests, builds both architectures, `lipo`s them into one universal app, signs it, packs the DMG, commits, tags, pushes, creates the release with `--latest`, updates the Homebrew tap, and checks that `/releases/latest` now resolves to it.

Releases are **ad-hoc signed** unless `CADUCEUS_SIGNING_IDENTITY` is set, in which case the DMG is also signed with a real Developer ID and notarized by Apple — see [Notarization](#notarization) at the end of this document.

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

Not required, and not currently used: an Apple Developer Program membership, a Developer ID certificate and notary credentials. With none of them a release is ad-hoc signed, which is the status quo. See [Notarization](#notarization).

## 1. Bump the version

Keep these four in sync (semver, e.g. `1.0.3`). The last one is the site's
`SoftwareApplication` structured data — search engines read it literally, so a
stale version there is worse than none:

| File | Field |
|------|--------|
| `package.json` | `"version"` |
| `src-tauri/tauri.conf.json` | `"version"` |
| `src-tauri/Cargo.toml` | `version = "…"` under `[package]` |
| `website/index.html` | `"softwareVersion"` in the schema.org block |

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
curl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash
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
curl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash
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

## Notarization

Caduceus ships **ad-hoc signed** today, and nothing below is required to cut a release. This section is what it would take to ship a signed, notarized build instead, and how to actually run it once you have the credentials. The plumbing already exists: [`scripts/notarize.sh`](./scripts/notarize.sh), called automatically by `npm run release` when `CADUCEUS_SIGNING_IDENTITY` is set and skipped with a one-line log when it is not.

### Why bother

An ad-hoc signature is a hash of the binary. It belongs to nobody and it is different in every build, which costs users two things:

- **The download is blocked.** Gatekeeper refuses an ad-hoc signed app that arrived from the internet. Right-click → Open, or `xattr -d com.apple.quarantine`, is the workaround people currently have to be told about.
- **Privacy grants go stale on every update.** macOS keys TCC entries to the code signature, so Accessibility granted to 2.3.0 does not apply to 2.4.0 — the switch in System Settings stays on while `AXIsProcessTrusted()` returns false. That is the entire reason the "repair" button and `tccutil reset` exist; see the doc comment on [`src-tauri/src/window/grants.rs`](./src-tauri/src/window/grants.rs). A Developer ID signature is stable across builds, so the grant survives the update and that button becomes dead weight.

### What you need, in order

**1. An Apple Developer Program membership.** [developer.apple.com/programs](https://developer.apple.com/programs/) — $99/year, individual or organization. A free Apple developer account is not enough: Developer ID certificates and the notary service are both paid-tier only. Enrolment takes anywhere from a day to a couple of weeks if they ask for documentation.

**2. A "Developer ID Application" certificate.** This is the specific certificate type for apps distributed outside the App Store; "Apple Development" and "Apple Distribution" are different things and will not notarize.

The easy route, in Xcode: **Settings → Accounts → your Apple ID → Manage Certificates → + → Developer ID Application**. It lands in your login keychain ready to use.

By hand: create a Certificate Signing Request in **Keychain Access → Certificate Assistant → Request a Certificate From a Certificate Authority** (saved to disk), upload it at [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates/list) choosing *Developer ID Application*, then download the `.cer` and double-click it.

Confirm it is installed and get the exact string to use:

```bash
security find-identity -v -p codesigning
```

The line you want reads `Developer ID Application: Your Name (ABCDE12345)`. The ten characters in parentheses are your Team ID.

**3. Notary credentials.** Two options; the API key is better and the script prefers it.

*App Store Connect API key (recommended).* At [appstoreconnect.apple.com/access/integrations/api](https://appstoreconnect.apple.com/access/integrations/api), Team Keys → **+** → role **Developer**. Download the `AuthKey_XXXXXXXXXX.p8` — **it can only be downloaded once**; store it somewhere durable and `chmod 600` it. You need three things from that page: the key file, the Key ID (the `XXXXXXXXXX` in the filename), and the Issuer ID (the UUID shown above the key list).

*Apple ID with an app-specific password.* At [account.apple.com](https://account.apple.com) → Sign-In and Security → **App-Specific Passwords** → generate one for "Caduceus notarization". It looks like `abcd-efgh-ijkl-mnop`. Your real Apple ID password will not work, and this route puts a secret in the process table for the duration of the submission, which is the main reason to prefer the key.

### The environment variables

| Variable | What it is |
|---|---|
| `CADUCEUS_SIGNING_IDENTITY` | **The switch.** The full identity string, e.g. `Developer ID Application: Your Name (ABCDE12345)`. Unset ⇒ ad-hoc release, everything below ignored |
| `CADUCEUS_NOTARY_KEY_PATH` | Path to `AuthKey_XXXXXXXXXX.p8` |
| `CADUCEUS_NOTARY_KEY_ID` | The Key ID, `XXXXXXXXXX` |
| `CADUCEUS_NOTARY_ISSUER_ID` | The Issuer UUID |
| `CADUCEUS_NOTARY_APPLE_ID` | Apple ID — *alternative* to the three above |
| `CADUCEUS_NOTARY_TEAM_ID` | Ten-character Team ID |
| `CADUCEUS_NOTARY_PASSWORD` | App-specific password |

Set the identity plus **one** complete trio. A partially-set trio is an error naming the missing variable, and an identity with no credentials at all is an error too — a Developer ID signature without a ticket still will not open on someone else's machine, so it is never what you meant.

```bash
export CADUCEUS_SIGNING_IDENTITY="Developer ID Application: Your Name (ABCDE12345)"
export CADUCEUS_NOTARY_KEY_PATH="$HOME/.private_keys/AuthKey_ABCD123456.p8"
export CADUCEUS_NOTARY_KEY_ID="ABCD123456"
export CADUCEUS_NOTARY_ISSUER_ID="69a6de70-0000-0000-0000-000000000000"

npm run release -- patch -m "notarized"
```

Keep these out of the repo — a shell profile, `direnv`, or `security add-generic-password`. There is no `.env` support and there should not be.

### What the script does

[`scripts/notarize.sh`](./scripts/notarize.sh) runs after the DMG is packed and before anything is committed or pushed, so a rejection costs a build rather than leaving a tag pointing at a DMG nobody can open. It can also be run by hand:

```bash
scripts/notarize.sh path/to/Caduceus.app path/to/Caduceus_2.4.0_universal.dmg
```

1. **Checks everything first** — `notarytool` is present, the identity is really in the keychain (it prints what *is* there if not), the credential trio is complete, the key file exists. Nothing is signed until all of it passes.
2. **Signs inside-out.** The four Swift helpers under `Contents/Resources/bin/` are signed before the bundle around them, because a signature seals what it contains and an outer one applied first is void the moment an inner one is rewritten. Each helper **keeps the identifier `build.rs` gave it** — TCC keys grants to that string, and both speech helpers deliberately share one so dictation prompts for the microphone once rather than twice.
3. **Hardened runtime, everywhere.** `--options runtime --timestamp --entitlements src-tauri/entitlements.plist`. The notary service rejects anything else. The entitlements file already carries `allow-jit` and `disable-library-validation` for Tauri's WKWebView, plus the audio-input entitlement the helpers need — under the hardened runtime, microphone access fails silently without it.
4. **Submits the `.app`** (zipped with `ditto -c -k --keepParent`; `notarytool` will not take a bare bundle) and waits, up to 30 minutes.
5. **Staples the ticket into the `.app`**, then **repacks the DMG** around the stapled copy. This step is the one that is easy to get wrong: a ticket stapled after the DMG was built is not *inside* the DMG, and the copy a user drags to `/Applications` is the copy inside the DMG. Skip it and the app only passes Gatekeeper while the machine is online.
6. **Signs, submits and staples the DMG itself**, so the disk image is trusted as well as its contents.
7. **Asks Gatekeeper** with `spctl --assess --type execute` on the app and `spctl --assess --type open --context context:primary-signature` on the DMG. The answer to look for is `source=Notarized Developer ID`.

Because the DMG is repacked, the Homebrew cask checksum — computed afterwards, from the release asset — covers the notarized bytes automatically.

### When it goes wrong

A rejection prints Apple's own reasons: the script pulls `xcrun notarytool log <submission-id>` and shows it. The usual causes are a nested binary that missed the hardened runtime, a missing secure timestamp (the machine was offline), or a `get-task-allow` entitlement left over from a debug build.

To inspect a build by hand:

```bash
codesign -dv --verbose=4 Caduceus.app          # Authority, TeamIdentifier, Timestamp, runtime flag
codesign --verify --deep --strict --verbose=2 Caduceus.app
xcrun stapler validate Caduceus.app            # is the ticket actually in there
spctl --assess --type execute -vv Caduceus.app # what Gatekeeper would decide
xcrun notarytool history --key … --key-id … --issuer …
```

### Also worth doing, once this is real

- Drop `signingIdentity: "-"` from `src-tauri/tauri.conf.json` so plain `npm run tauri build` signs properly too — the release script signs over it either way.
- Revisit the "repair permissions" button. It stops being necessary once signatures are stable, though leaving it costs nothing and still helps anyone on an older ad-hoc build.
- The installer and the cask need no changes. Notarization is invisible to both; users simply stop being told to right-click.

## Related docs

- [README.md](./README.md) — development build and local install
- [website/install.sh](./website/install.sh) — what users run (canonical URL is the GitHub raw link in the script header; redeploy the Worker when the script changes)

### Install script and Cloudflare

The public one-liner uses `raw.githubusercontent.com/.../website/install.sh` so **curl** is never blocked by a browser-only challenge on `vivaanshahani.com`. If you want the vanity URL to work from Terminal too, add a Cloudflare WAF **Skip** rule for paths ending in `install.sh` on `caduceus.vivaanshahani.com` and `vivaanshahani.com/caduceus/install.sh`.
