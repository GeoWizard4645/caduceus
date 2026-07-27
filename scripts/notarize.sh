#!/usr/bin/env bash
#
# Sign a Caduceus build with a real Developer ID and notarize it with Apple.
#
#   scripts/notarize.sh <path/to/Caduceus.app> [path/to/Caduceus.dmg]
#
# `npm run release` calls this straight after the DMG is packed, but only when
# CADUCEUS_SIGNING_IDENTITY is set. With no identity there is nothing to do and
# nothing happens: the ad-hoc release flow is exactly what it always was.
#
# # What notarization buys
#
# An ad-hoc signature is a hash of the binary, so it is different in every
# build and belongs to nobody. Gatekeeper will not open a downloaded app that
# carries one, and macOS keys TCC privacy grants to the signature, which is why
# Accessibility silently stops working after an update and why the "repair"
# button in the permission page exists at all (see src/window/grants.rs).
#
# A Developer ID signature is stable across builds and belongs to one team, so
# both problems go away: the download opens without the right-click dance, and
# a grant given to 2.4.0 is still valid in 2.5.0.
#
# # What it does, in the only order that works
#
# 1. Signs every nested Mach-O first, then the bundle around them. Signatures
#    seal what they contain, so an outer signature applied before an inner one
#    is invalid the moment the inner one is written.
# 2. Submits the .app and waits for the ticket.
# 3. Staples the ticket into the .app.
# 4. **Repacks the DMG** from the stapled app. A ticket stapled after the DMG
#    was built is not inside the DMG, and the copy a user drags to
#    /Applications is the copy inside the DMG. Skipping this step gives you an
#    app that only passes Gatekeeper while the machine is online.
# 5. Submits and staples the DMG itself, so the disk image is trusted too.
# 6. Asks Gatekeeper, via spctl, whether it would actually let this run.
#
# # Credentials
#
# The signing identity, always:
#
#   CADUCEUS_SIGNING_IDENTITY   "Developer ID Application: Name (TEAMID)"
#
# Plus one of the two ways to talk to the notary service. An App Store Connect
# API key, which is the better one — it does not put a password in the process
# table and it does not expire the way an app-specific password can:
#
#   CADUCEUS_NOTARY_KEY_PATH    path to the AuthKey_XXXXXXXXXX.p8
#   CADUCEUS_NOTARY_KEY_ID      the key ID, the XXXXXXXXXX in that filename
#   CADUCEUS_NOTARY_ISSUER_ID   the issuer UUID from App Store Connect
#
# Or an Apple ID with an app-specific password:
#
#   CADUCEUS_NOTARY_APPLE_ID    the Apple ID the certificate belongs to
#   CADUCEUS_NOTARY_TEAM_ID     the ten-character team ID
#   CADUCEUS_NOTARY_PASSWORD    an app-specific password, *not* the real one
#
# RELEASE.md walks through obtaining all of these from scratch.

set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

readonly BUNDLE_ID="com.caduceus.desktop"
readonly ENTITLEMENTS="$ROOT/src-tauri/entitlements.plist"
readonly VOLNAME="Caduceus"

# --- output ----------------------------------------------------------------

if [[ -t 1 ]]; then
  readonly DIM=$'\033[2m' BOLD=$'\033[1m' RED=$'\033[31m' GREEN=$'\033[32m' OFF=$'\033[0m'
else
  readonly DIM="" BOLD="" RED="" GREEN="" OFF=""
fi

step() { printf '\n%s==>%s %s%s%s\n' "$GREEN" "$OFF" "$BOLD" "$*" "$OFF"; }
note() { printf '    %s%s%s\n' "$DIM" "$*" "$OFF"; }
warn() { printf '    %swarning:%s %s\n' "$RED" "$OFF" "$*" >&2; }
die()  { printf '\n%serror:%s %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }

# --- arguments -------------------------------------------------------------

APP="${1:-}"
DMG="${2:-}"

[[ -n "$APP" ]] || die "usage: scripts/notarize.sh <Caduceus.app> [Caduceus.dmg]"

# --- the escape hatch ------------------------------------------------------

# The whole point of the env var: with no identity this is a no-op that exits
# clean, so release.sh can call it unconditionally and today's flow is
# unchanged. Not an error, and not a warning either — it is the normal case.
if [[ -z "${CADUCEUS_SIGNING_IDENTITY:-}" ]]; then
  note "ad-hoc signed — set CADUCEUS_SIGNING_IDENTITY to notarize"
  exit 0
fi

readonly IDENTITY="$CADUCEUS_SIGNING_IDENTITY"

# --- preflight -------------------------------------------------------------

# Everything is checked before anything is signed. A missing issuer ID should
# cost you a second, not leave a half-signed bundle and a stale DMG behind.

step "Checking the identity and the credentials"

command -v xcrun    >/dev/null || die "xcrun is missing — xcode-select --install"
command -v codesign >/dev/null || die "codesign is missing — xcode-select --install"
command -v ditto    >/dev/null || die "ditto is missing — xcode-select --install"

xcrun --find notarytool >/dev/null 2>&1 \
  || die "notarytool is missing. It needs Xcode 13+; the standalone Command Line Tools do not always carry it."

[[ -d "$APP" ]] || die "no app bundle at $APP"
[[ -f "$ENTITLEMENTS" ]] || die "no entitlements at $ENTITLEMENTS"
[[ -z "$DMG" || -f "$DMG" ]] || die "no DMG at $DMG"

if ! security find-identity -v -p codesigning 2>/dev/null | grep -qF -- "$IDENTITY"; then
  printf '\n' >&2
  warn "no codesigning identity in the keychain matches:"
  note "  $IDENTITY"
  note "What is there:"
  security find-identity -v -p codesigning 2>/dev/null | sed 's/^/      /' >&2
  die "install the \"Developer ID Application\" certificate, or fix CADUCEUS_SIGNING_IDENTITY (RELEASE.md § Notarization)"
fi

note "identity: $IDENTITY"

# notarytool takes its credentials as flags either way; which set you have
# decides which flags. The API key comes first because it is the better one.
NOTARY_AUTH=()
if [[ -n "${CADUCEUS_NOTARY_KEY_PATH:-}" || -n "${CADUCEUS_NOTARY_KEY_ID:-}" || -n "${CADUCEUS_NOTARY_ISSUER_ID:-}" ]]; then
  [[ -n "${CADUCEUS_NOTARY_KEY_PATH:-}"  ]] || die "CADUCEUS_NOTARY_KEY_PATH is not set (the other API-key variables are)"
  [[ -n "${CADUCEUS_NOTARY_KEY_ID:-}"    ]] || die "CADUCEUS_NOTARY_KEY_ID is not set (the other API-key variables are)"
  [[ -n "${CADUCEUS_NOTARY_ISSUER_ID:-}" ]] || die "CADUCEUS_NOTARY_ISSUER_ID is not set (the other API-key variables are)"
  [[ -f "$CADUCEUS_NOTARY_KEY_PATH" ]] || die "no API key file at $CADUCEUS_NOTARY_KEY_PATH"
  NOTARY_AUTH=(--key "$CADUCEUS_NOTARY_KEY_PATH"
               --key-id "$CADUCEUS_NOTARY_KEY_ID"
               --issuer "$CADUCEUS_NOTARY_ISSUER_ID")
  note "notary auth: App Store Connect API key $CADUCEUS_NOTARY_KEY_ID"
elif [[ -n "${CADUCEUS_NOTARY_APPLE_ID:-}" || -n "${CADUCEUS_NOTARY_TEAM_ID:-}" || -n "${CADUCEUS_NOTARY_PASSWORD:-}" ]]; then
  [[ -n "${CADUCEUS_NOTARY_APPLE_ID:-}" ]] || die "CADUCEUS_NOTARY_APPLE_ID is not set (the other Apple-ID variables are)"
  [[ -n "${CADUCEUS_NOTARY_TEAM_ID:-}"  ]] || die "CADUCEUS_NOTARY_TEAM_ID is not set (the other Apple-ID variables are)"
  [[ -n "${CADUCEUS_NOTARY_PASSWORD:-}" ]] || die "CADUCEUS_NOTARY_PASSWORD is not set (the other Apple-ID variables are)"
  NOTARY_AUTH=(--apple-id "$CADUCEUS_NOTARY_APPLE_ID"
               --team-id "$CADUCEUS_NOTARY_TEAM_ID"
               --password "$CADUCEUS_NOTARY_PASSWORD")
  note "notary auth: Apple ID $CADUCEUS_NOTARY_APPLE_ID"
else
  printf '\n' >&2
  warn "CADUCEUS_SIGNING_IDENTITY is set, so this build is meant to be notarized, but there is nothing to notarize it with."
  note "Set either the API key trio:"
  note "  CADUCEUS_NOTARY_KEY_PATH  CADUCEUS_NOTARY_KEY_ID  CADUCEUS_NOTARY_ISSUER_ID"
  note "or the Apple ID trio:"
  note "  CADUCEUS_NOTARY_APPLE_ID  CADUCEUS_NOTARY_TEAM_ID  CADUCEUS_NOTARY_PASSWORD"
  die "see RELEASE.md § Notarization. Unset CADUCEUS_SIGNING_IDENTITY to cut an ad-hoc release instead."
fi

WORK="$(mktemp -d -t caduceus-notarize)"
trap 'rm -rf "$WORK"' EXIT

# --- signing ---------------------------------------------------------------

step "Signing $APP"

# `--options runtime` is not optional: the notary service rejects any
# executable that is not built against the hardened runtime. `--timestamp`
# is not optional either — a signature without a secure timestamp cannot be
# notarized, and it needs the network.
#
# The helpers get the same entitlements as the app because the hardened runtime
# is what makes them necessary: without com.apple.security.device.audio-input
# the microphone call fails inside caduceus-stt rather than prompting.
sign() {
  codesign --force --timestamp --options runtime \
    --entitlements "$ENTITLEMENTS" \
    --identifier "$1" \
    --sign "$IDENTITY" \
    "$2"
}

# Inner binaries first. A code signature seals everything below it, so signing
# the bundle and then touching a helper inside it invalidates the bundle.
#
# Each helper keeps the identifier build.rs gave it rather than inheriting the
# app's. TCC keys privacy grants to that string, and the speech helpers
# deliberately share one so that dictation asks for the microphone once instead
# of twice — see src-tauri/build.rs.
while IFS= read -r -d '' helper; do
  file -b "$helper" | grep -q 'Mach-O' || continue
  existing="$(codesign -d --verbose=2 "$helper" 2>&1 | sed -n 's/^Identifier=//p' | head -1)"
  [[ -n "$existing" ]] || existing="$BUNDLE_ID.$(basename "$helper")"
  sign "$existing" "$helper"
  note "$(basename "$helper") → $existing"
done < <(find "$APP/Contents/Resources" "$APP/Contents/Frameworks" -type f -print0 2>/dev/null)

sign "$BUNDLE_ID" "$APP"
note "$(basename "$APP") → $BUNDLE_ID"

codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | sed 's/^/      /'

# --- submit the app --------------------------------------------------------

# notarytool takes a .zip, a .dmg or a .pkg, never a bare bundle. `ditto -c -k
# --keepParent` is the archiver Apple documents for this; `zip` does not
# preserve the symlinks and extended attributes a bundle relies on.
step "Submitting the app to Apple"
note "this takes a few minutes; Apple's queue decides how many"

readonly APP_ZIP="$WORK/Caduceus.zip"
ditto -c -k --keepParent "$APP" "$APP_ZIP"

# `--wait` blocks until the verdict is in. `--timeout` stops a stuck queue from
# hanging a release forever; the submission is unaffected and can be polled
# later with `xcrun notarytool info <id>`.
submit() {
  local target="$1" log="$WORK/notarytool.log"

  # `|| true` so that a rejection is reported by the log-fetching branch below
  # rather than by `set -e` killing the script one line before the reasons.
  xcrun notarytool submit "$target" "${NOTARY_AUTH[@]}" --wait --timeout 30m 2>&1 \
    | tee "$log" | sed 's/^/      /' || true

  local id
  id="$(sed -n 's/^ *id: *//p' "$log" | head -1)"

  if ! grep -q 'status: Accepted' "$log"; then
    if [[ -n "$id" ]]; then
      printf '\n' >&2
      warn "Apple rejected $target. The reasons:"
      xcrun notarytool log "$id" "${NOTARY_AUTH[@]}" 2>&1 | sed 's/^/      /' >&2 || true
    fi
    die "notarization failed for $target"
  fi

  note "accepted${id:+ (submission $id)}"
}

submit "$APP_ZIP"

step "Stapling the ticket into the app"
xcrun stapler staple "$APP" | sed 's/^/      /'

# --- the DMG ---------------------------------------------------------------

if [[ -z "$DMG" ]]; then
  step "No DMG given — the app is signed, notarized and stapled"
  spctl --assess --type execute -vv "$APP" 2>&1 | sed 's/^/      /'
  exit 0
fi

# The DMG on disk still contains the app as it was *before* the ticket was
# stapled into it, and that copy is the one people drag to /Applications. Rebuilt
# from the same staging directory release.sh packed it from, so the only
# difference is the ticket.
step "Repacking the DMG around the stapled app"
readonly STAGE="$(dirname "$APP")"
rm -f "$DMG"
hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" -quiet
note "$DMG ($(du -h "$DMG" | cut -f1))"

# Signing the disk image as well is what makes `spctl --type open` meaningful:
# without it the image is unsigned baggage around a signed app.
codesign --force --timestamp --sign "$IDENTITY" "$DMG"

step "Submitting the DMG to Apple"
submit "$DMG"

step "Stapling the ticket into the DMG"
xcrun stapler staple "$DMG" | sed 's/^/      /'

# --- verify ----------------------------------------------------------------

# The only question that matters: would Gatekeeper open this on a machine that
# has never seen it? "source=Notarized Developer ID" is the answer to look for.
step "Asking Gatekeeper"

if spctl --assess --type execute -vv "$APP" 2>&1 | sed 's/^/      /'; then
  note "the app would open on a clean machine"
else
  die "Gatekeeper rejected the app. It is signed and stapled, so something above did not take."
fi

# A DMG is assessed as a document being opened, not as code being executed, and
# the primary-signature context is the one Gatekeeper uses for a download.
if spctl --assess --type open --context context:primary-signature -vv "$DMG" 2>&1 | sed 's/^/      /'; then
  note "the DMG would mount without a warning"
else
  die "Gatekeeper rejected the DMG."
fi

step "Notarized"
note "$APP"
note "$DMG"
