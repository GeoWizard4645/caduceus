#!/bin/bash
#
# Caduceus installer.
#
#   curl -fsSL https://vivaanshahani.com/caduceus/install.sh | bash
#
# With no flags this installs Caduceus alone: downloads the latest release DMG,
# copies the app to /Applications, clears the quarantine flag, and launches it.
#
# The configured-package flow adds optional pieces on top:
#
#   ... | bash -s -- --with=caduceus,hermes,ollama \
#                    --pull=qwen3.5:4b,qwen2.5vl:7b \
#                    --ai=qwen3.5:4b --hermes-model=qwen2.5vl:7b
#
#   --with=a,b,c          components: deps, caduceus, hermes, ollama
#                         "deps" = Xcode Command Line Tools + python3, and is
#                         added automatically whenever hermes or ollama is asked
#                         for, since neither works without them
#   --pull=x,y            Ollama models to pull (implies ollama)
#   --ai=MODEL            wire MODEL to Caduceus `/` via localhost:11434/v1
#   --hermes-model=MODEL  point Hermes at MODEL on Ollama, enable computer_use,
#                         set Caduceus `/c` to Hermes (implies hermes)
#   --computer-use=MODEL  legacy: wire MODEL to `/c` as a vision backend (no Hermes)
#
# Everything it does is visible below — read it before running it, as you should
# with any script piped into a shell.

set -euo pipefail

REPO="GeoWizard4645/caduceus"
APP_NAME="Caduceus"
INSTALL_DIR="/Applications"
BUNDLE_ID="com.caduceus.desktop"
OLLAMA_URL="http://localhost:11434/v1"

bold=$(tput bold 2>/dev/null || echo "")
dim=$(tput dim 2>/dev/null || echo "")
red=$(tput setaf 1 2>/dev/null || echo "")
green=$(tput setaf 2 2>/dev/null || echo "")
reset=$(tput sgr0 2>/dev/null || echo "")

say()  { echo "${bold}==>${reset} $*"; }
warn() { echo "${red}==>${reset} $*" >&2; }
die()  { warn "$*"; exit 1; }

# --- arguments --------------------------------------------------------------

with=""
pull=""
ai_model=""
cu_model=""
hermes_model=""

for arg in "$@"; do
  case "$arg" in
    --with=*)           with="${arg#*=}" ;;
    --pull=*)           pull="${arg#*=}" ;;
    --ai=*)             ai_model="${arg#*=}" ;;
    --computer-use=*)   cu_model="${arg#*=}" ;;
    --hermes-model=*)   hermes_model="${arg#*=}" ;;
    -h|--help)          sed -n '3,27p' "$0" 2>/dev/null || true; exit 0 ;;
    *) die "Unknown option: $arg" ;;
  esac
done

# Hermes-backed screen control needs the agent installed.
[ -z "$hermes_model" ] || case ",$with," in *,hermes,*) ;; *) with="$with,hermes" ;; esac

# No --with at all means the plain one-liner, whose contract is "install the app".
[ -n "$with" ] || with="caduceus"

# Models are useless without the runtime that serves them.
[ -z "$pull" ] || case ",$with," in *,ollama,*) ;; *) with="$with,ollama" ;; esac

wants() { case ",$with," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

# Ollama and Hermes both need a working toolchain, and wiring Caduceus's config
# needs python3. Implied rather than optional: a run that installs either one
# without them fails partway through, which is worse than taking a minute here.
if wants ollama || wants hermes; then
  case ",$with," in *,deps,*) ;; *) with="deps,$with" ;; esac
fi

# --- preflight --------------------------------------------------------------

[ "$(uname -s)" = "Darwin" ] || die "Caduceus is macOS-only. This looks like $(uname -s)."

major=$(sw_vers -productVersion | cut -d. -f1)
[ "$major" -ge 11 ] || die "Caduceus needs macOS 11 or newer (found $(sw_vers -productVersion))."

command -v curl >/dev/null || die "curl is required."

# --- required tools ---------------------------------------------------------
#
# The Xcode Command Line Tools are the one package that covers everything the
# rest of this script leans on: python3 (used to write Caduceus's settings),
# git, and a compiler for anything Hermes builds from source. Installing them
# is a GUI flow Apple owns — `xcode-select --install` opens a window and returns
# immediately — so this polls rather than assuming it finished.

install_deps() {
  if xcode-select -p >/dev/null 2>&1; then
    say "Xcode Command Line Tools are already installed."
  else
    say "Installing the Xcode Command Line Tools (~1.5 GB)…"
    warn "Apple's installer window will open. Accept it, then leave this running."
    xcode-select --install >/dev/null 2>&1 || true

    local waited=0
    until xcode-select -p >/dev/null 2>&1; do
      sleep 5
      waited=$((waited + 5))
      if [ "$waited" -ge 1800 ]; then
        die "Timed out waiting for the Command Line Tools.
Finish that installer (or run: xcode-select --install), then re-run this command — it picks up where it left off."
      fi
    done
    say "Command Line Tools installed."
  fi

  # Belt and braces: some machines report a valid developer dir but have no
  # usable python3, and finding that out during configure_models is too late.
  if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is still missing after installing the Command Line Tools.
Install it any way you like (e.g. brew install python), then re-run this command."
  fi
  say "python3 is available ($(python3 --version 2>&1))."
}

# --- Caduceus ---------------------------------------------------------------

install_caduceus() {
  # Apple Silicon and Intel get different builds.
  local arch
  case "$(uname -m)" in
    arm64) arch="aarch64" ;;
    x86_64) arch="x64" ;;
    *) die "Unsupported architecture: $(uname -m)" ;;
  esac

  say "Looking up the latest release…"
  local api="https://api.github.com/repos/${REPO}/releases/latest"

  # Pick the DMG matching this architecture, falling back to any DMG in the
  # release for single-build releases.
  local dmg_url
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

  local tmp
  tmp=$(mktemp -d)
  # Always clean up: leaving a mounted volume behind is worse than failing.
  mount_point="$tmp/mnt"
  cleanup() {
    if [ -n "${mount_point:-}" ] && [ -d "${mount_point:-}" ]; then
      hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
    fi
    rm -rf "$tmp"
  }
  trap cleanup EXIT

  local dmg="$tmp/caduceus.dmg"
  say "Downloading $(basename "$dmg_url")…"
  curl -fSL --progress-bar "$dmg_url" -o "$dmg"

  say "Mounting…"
  mkdir -p "$mount_point"
  hdiutil attach "$dmg" -nobrowse -quiet -mountpoint "$mount_point"

  local source_app="$mount_point/${APP_NAME}.app"
  [ -d "$source_app" ] || die "${APP_NAME}.app was not in the disk image."

  local target="${INSTALL_DIR}/${APP_NAME}.app"
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
}

# --- Ollama -----------------------------------------------------------------

install_ollama() {
  if command -v ollama >/dev/null 2>&1; then
    say "Ollama is already installed."
  else
    say "Installing Ollama…"
    curl -fsSL https://ollama.com/install.sh | sh
  fi

  # `ollama pull` needs the server up; starting it is idempotent.
  if ! curl -fsS --max-time 2 http://localhost:11434/api/version >/dev/null 2>&1; then
    say "Starting the Ollama server…"
    (ollama serve >/dev/null 2>&1 &)
    for _ in $(seq 1 30); do
      curl -fsS --max-time 1 http://localhost:11434/api/version >/dev/null 2>&1 && break
      sleep 1
    done
  fi
}

pull_models() {
  local IFS=,
  for model in $pull; do
    [ -n "$model" ] || continue
    say "Pulling ${model}… (this is the slow part)"
    # One bad tag must not abort the models that follow it.
    ollama pull "$model" || warn "Could not pull ${model}. Check the tag at ollama.com/library."
  done
}

# --- Hermes Agent -----------------------------------------------------------

install_hermes() {
  if command -v hermes >/dev/null 2>&1; then
    say "Hermes Agent is already installed."
  else
    say "Installing Hermes Agent…"
    curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash
  fi
}

configure_hermes() {
  [ -n "$hermes_model" ] || return 0
  command -v hermes >/dev/null 2>&1 || {
    warn "Hermes is not on PATH — skipping Hermes model and computer_use setup."
    return 0
  }

  say "Pointing Hermes at Ollama (${hermes_model})…"
  hermes config set model.provider custom 2>/dev/null || \
    warn "Could not set Hermes provider — run \`hermes model\` manually."
  hermes config set model.base_url "$OLLAMA_URL" 2>/dev/null || true
  hermes config set model.default "$hermes_model" 2>/dev/null || \
    warn "Could not set Hermes model — run \`hermes model\` manually."

  say "Enabling Hermes computer_use…"
  hermes tools enable computer_use 2>/dev/null || \
    warn "Could not enable computer_use — run \`hermes tools enable computer_use\`."

  say "Installing the Hermes screen-control driver (may prompt for permissions)…"
  hermes computer-use install 2>/dev/null || \
    warn "Screen driver install had issues — try \`hermes computer-use doctor\` after granting Accessibility and Screen Recording."
}

# --- wiring the models into Caduceus ----------------------------------------
#
# Caduceus keeps its config as one JSON blob under "settings" in
# ~/Library/Application Support/<bundle id>/caduceus-settings.json. Editing it
# needs a real JSON parser, and macOS has no guaranteed jq — python3 ships with
# the Command Line Tools, which most machines running this will have. When it is
# missing we say so and print what to do by hand rather than corrupting the file
# with sed.

configure_models() {
  [ -n "$ai_model$cu_model$hermes_model" ] || return 0

  local cfg="$HOME/Library/Application Support/${BUNDLE_ID}/caduceus-settings.json"

  # install_deps guarantees this whenever Ollama or Hermes is in play, so
  # reaching here without it means someone passed --ai with neither. Fail rather
  # than report success over a config that was never written.
  if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required to wire the models into Caduceus, and it is not installed.
Re-run with --with=deps to install it, or add the backend by hand:
Settings → AI → new OpenAI-compatible backend at ${OLLAMA_URL}."
  fi

  say "Wiring models into Caduceus…"
  use_hermes_cu=0
  [ -n "$hermes_model" ] && use_hermes_cu=1

  AI_MODEL="$ai_model" CU_MODEL="$cu_model" USE_HERMES_CU="$use_hermes_cu" \
    CFG="$cfg" BASE_URL="$OLLAMA_URL" python3 <<'PY'
import json, os, pathlib

cfg = pathlib.Path(os.environ["CFG"])
ai, cu, base = os.environ["AI_MODEL"], os.environ["CU_MODEL"], os.environ["BASE_URL"]
use_hermes = os.environ.get("USE_HERMES_CU") == "1"

cfg.parent.mkdir(parents=True, exist_ok=True)
try:
    store = json.loads(cfg.read_text())
except (OSError, ValueError):
    store = {}

settings = store.setdefault("settings", {})
agents = settings.setdefault("agents", {})
backends = agents.setdefault("backends", [])


def ensure_hermes():
    for entry in backends:
        if entry.get("id") == "hermes":
            entry.update({
                "displayName": "Hermes Agent",
                "kind": "hermes",
                "supportsComputerUse": True,
            })
            return
    backends.insert(0, {
        "id": "hermes",
        "displayName": "Hermes Agent",
        "kind": "hermes",
        "baseUrl": "",
        "model": "",
        "hasApiKey": False,
        "supportsComputerUse": True,
        "maxTokens": 4096,
        "timeoutSecs": 600,
    })


def upsert_ollama(backend_id, name, model, computer_use):
    for b in backends:
        if b.get("id") == backend_id:
            entry = b
            break
    else:
        entry = {"id": backend_id}
        backends.append(entry)
    entry.update({
        "displayName": name,
        "kind": "openai_compatible",
        "baseUrl": base,
        "model": model,
        "hasApiKey": False,
        "supportsComputerUse": computer_use,
        "maxTokens": 4096,
        "timeoutSecs": 600,
    })
    return backend_id


if ai:
    agents["primaryBackendId"] = upsert_ollama("ollama-chat", f"Ollama — {ai}", ai, False)

if use_hermes:
    ensure_hermes()
    agents["computerUseBackendId"] = "hermes"
elif cu:
    agents["computerUseBackendId"] = upsert_ollama(
        "ollama-vision", f"Ollama — {cu}", cu, True
    )

cfg.write_text(json.dumps(store, indent=2))
print(f"  wrote {cfg}")
PY
}

# --- run --------------------------------------------------------------------

wants deps     && install_deps
wants caduceus && install_caduceus
wants ollama   && install_ollama
[ -z "$pull" ] || pull_models
wants hermes   && install_hermes
configure_hermes
configure_models

if wants caduceus; then
  say "Launching…"
  open "${INSTALL_DIR}/${APP_NAME}.app"
fi

cat <<EOF

${green}${bold}Done.${reset}

  ${dim}Look for the caduceus in your menu bar — there is no Dock icon.${reset}

  ${bold}Control+Space${reset}   open the Command Center
  ${bold}F12${reset}             hide or show the floating staff
  ${bold}Alt+Shift+V${reset}     hold to talk

  Type an app name to launch it, or maths to calculate it.

EOF

if [ -n "$ai_model" ] || [ -n "$cu_model" ] || [ -n "$hermes_model" ]; then
  cat <<EOF
${dim}Wired up:${reset}
${ai_model:+  ${bold}/${reset}   Ollama · ${OLLAMA_URL} · ${ai_model}
}${hermes_model:+  ${bold}/c${reset}  Hermes computer_use · Ollama · ${hermes_model}
}${cu_model:+  ${bold}/c${reset}  Ollama vision · ${cu_model}
}
${dim}Restart Caduceus if it was already running when this finished.${reset}

EOF
elif ! command -v hermes >/dev/null 2>&1; then
  cat <<EOF
${dim}For the / and /c AI prefixes, install Hermes Agent:${reset}
  curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash
  hermes setup --portal

EOF
fi
