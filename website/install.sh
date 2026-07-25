#!/bin/bash
#
# Caduceus installer.
#
#   curl -fsSL https://vivaanshahani.com/caduceus/install.sh | bash
#
# Downloads the latest release DMG, copies the app to /Applications, clears the
# quarantine flag, and launches it. Everything it does is visible below — read
# it before running it, as you should with any script piped into a shell.

set -euo pipefail

REPO="GeoWizard4645/caduceus"
APP_NAME="Caduceus"
INSTALL_DIR="/Applications"

bold=$(tput bold 2>/dev/null || echo "")
dim=$(tput dim 2>/dev/null || echo "")
red=$(tput setaf 1 2>/dev/null || echo "")
green=$(tput setaf 2 2>/dev/null || echo "")
reset=$(tput sgr0 2>/dev/null || echo "")

say()  { echo "${bold}==>${reset} $*"; }
warn() { echo "${red}==>${reset} $*" >&2; }
die()  { warn "$*"; exit 1; }

# --- preflight --------------------------------------------------------------

[ "$(uname -s)" = "Darwin" ] || die "Caduceus is macOS-only. This looks like $(uname -s)."

major=$(sw_vers -productVersion | cut -d. -f1)
[ "$major" -ge 11 ] || die "Caduceus needs macOS 11 or newer (found $(sw_vers -productVersion))."

command -v curl >/dev/null || die "curl is required."

# Apple Silicon and Intel get different builds.
case "$(uname -m)" in
  arm64) arch="aarch64" ;;
  x86_64) arch="x64" ;;
  *) die "Unsupported architecture: $(uname -m)" ;;
esac

# --- find the latest release ------------------------------------------------

say "Looking up the latest release…"
api="https://api.github.com/repos/${REPO}/releases/latest"

# Pick the DMG matching this architecture, falling back to any DMG in the
# release for single-build releases.
dmg_url=$(curl -fsSL "$api" \
  | grep -o '"browser_download_url": *"[^"]*\.dmg"' \
  | cut -d'"' -f4 \
  | grep -i "$arch" \
  | head -1 || true)

if [ -z "$dmg_url" ]; then
  dmg_url=$(curl -fsSL "$api" \
    | grep -o '"browser_download_url": *"[^"]*\.dmg"' \
    | cut -d'"' -f4 \
    | head -1 || true)
fi

[ -n "$dmg_url" ] || die "No .dmg found in the latest release of ${REPO}.
Build it yourself instead:  git clone https://github.com/${REPO} && cd caduceus && npm install && npm run bundle"

# --- download ---------------------------------------------------------------

tmp=$(mktemp -d)
# Always clean up: leaving a mounted volume behind is worse than failing.
cleanup() {
  if [ -n "${mount_point:-}" ] && [ -d "${mount_point:-}" ]; then
    hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

dmg="$tmp/caduceus.dmg"
say "Downloading $(basename "$dmg_url")…"
curl -fSL --progress-bar "$dmg_url" -o "$dmg"

# --- install ----------------------------------------------------------------

say "Mounting…"
mount_point="$tmp/mnt"
mkdir -p "$mount_point"
hdiutil attach "$dmg" -nobrowse -quiet -mountpoint "$mount_point"

source_app="$mount_point/${APP_NAME}.app"
[ -d "$source_app" ] || die "${APP_NAME}.app was not in the disk image."

target="${INSTALL_DIR}/${APP_NAME}.app"
if [ -d "$target" ]; then
  say "Replacing the existing install…"
  # Quit a running copy first, or the replaced binary keeps running.
  osascript -e "tell application \"${APP_NAME}\" to quit" >/dev/null 2>&1 || true
  pkill -f "${APP_NAME}.app/Contents/MacOS/" >/dev/null 2>&1 || true
  sleep 1
  rm -rf "$target"
fi

say "Installing to ${INSTALL_DIR}…"
if ! cp -R "$source_app" "$target" 2>/dev/null; then
  warn "Need permission to write to ${INSTALL_DIR}."
  sudo cp -R "$source_app" "$target"
fi

# Caduceus is not notarised yet, so macOS would otherwise refuse to open it.
say "Clearing the quarantine flag…"
xattr -dr com.apple.quarantine "$target" 2>/dev/null || \
  sudo xattr -dr com.apple.quarantine "$target" 2>/dev/null || \
  warn "Could not clear quarantine. Run: xattr -dr com.apple.quarantine \"$target\""

# --- done -------------------------------------------------------------------

say "Launching…"
open "$target"

cat <<EOF

${green}${bold}Caduceus is installed.${reset}

  ${dim}Look for the caduceus in your menu bar — there is no Dock icon.${reset}

  ${bold}Alt+Space${reset}   open the Command Center
  ${bold}F12${reset}         hide or show the floating staff
  ${bold}⌘⇧Space${reset}     hold to talk

  Type an app name to launch it, or maths to calculate it.

${dim}For the / and /c AI prefixes, install Hermes Agent:${reset}
  curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash
  hermes setup --portal

EOF
