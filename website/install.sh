#!/bin/bash
#
# Caduceus installer.
#
#   curl -fsSL https://vivaanshahani.com/caduceus/install.sh | bash
#
# With no flags this installs Caduceus alone: downloads the universal .dmg from
# the latest release, copies the app to /Applications, clears the quarantine
# flag, and launches it. About 10 MB and ten seconds.
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
#   --from-source         compile Caduceus here instead of downloading it. Slow
#                         (~700 MB of Rust/Node toolchain, a few minutes) and
#                         needed by almost nobody — it exists for people who want
#                         to run only code they built, and as a way out if a
#                         release is ever missing or broken.
#   --rebuild             with --from-source, rebuild even if already up to date
#
# Everything it does is visible below — read it before running it, as you should
# with any script piped into a shell.

set -euo pipefail

REPO="GeoWizard4645/caduceus"
APP_NAME="Caduceus"
INSTALL_DIR="/Applications"
BUNDLE_ID="com.caduceus.desktop"
OLLAMA_URL="http://localhost:11434/v1"

# Everything this script owns lives here, so uninstalling is `rm -rf`.
CADUCEUS_HOME="$HOME/.caduceus"
SRC_DIR="$CADUCEUS_HOME/src"
TOOLS_DIR="$CADUCEUS_HOME/toolchain"
STAMP="$CADUCEUS_HOME/installed-commit"

bold=$(tput bold 2>/dev/null || echo "")
dim=$(tput dim 2>/dev/null || echo "")
red=$(tput setaf 1 2>/dev/null || echo "")
green=$(tput setaf 2 2>/dev/null || echo "")
reset=$(tput sgr0 2>/dev/null || echo "")

say()  { echo "${bold}==>${reset} $*"; }
warn() { echo "${red}==>${reset} $*" >&2; }
die()  { warn "$*"; exit 1; }

# Set by the download path and read by the EXIT trap. Leaving a disk image
# mounted is worse than failing, so cleanup runs however the script ends —
# including the die() paths between attach and detach.
tmp_dir=""
mount_point=""

cleanup() {
  if [ -n "$mount_point" ] && [ -d "$mount_point" ]; then
    hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
  fi
  [ -z "$tmp_dir" ] || rm -rf "$tmp_dir"
  # Never let cleanup itself decide the script's exit status.
  return 0
}

# --- arguments --------------------------------------------------------------

with=""
pull=""
ai_model=""
cu_model=""
hermes_model=""
from_source=0
rebuild=0

for arg in "$@"; do
  case "$arg" in
    --with=*)           with="${arg#*=}" ;;
    --pull=*)           pull="${arg#*=}" ;;
    --ai=*)             ai_model="${arg#*=}" ;;
    --computer-use=*)   cu_model="${arg#*=}" ;;
    --hermes-model=*)   hermes_model="${arg#*=}" ;;
    --from-source)      from_source=1 ;;
    --rebuild)          from_source=1; rebuild=1 ;;
    -h|--help)          sed -n '3,34p' "$0" 2>/dev/null || true; exit 0 ;;
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
# needs python3. Compiling Caduceus additionally needs swiftc and git, which the
# same package provides. Implied rather than optional: a run that installs any of
# them without it fails partway through, which is worse than taking a minute
# here. The plain download path needs none of this, which is the point of it.
if wants ollama || wants hermes || { [ "$from_source" -eq 1 ] && wants caduceus; }; then
  case ",$with," in *,deps,*) ;; *) with="deps,$with" ;; esac
fi

# --- preflight --------------------------------------------------------------

[ "$(uname -s)" = "Darwin" ] || die "Caduceus is macOS-only. This looks like $(uname -s)."

major=$(sw_vers -productVersion | cut -d. -f1)
[ "$major" -ge 11 ] || die "Caduceus needs macOS 11 or newer (found $(sw_vers -productVersion))."

command -v curl >/dev/null || die "curl is required."

# --- required tools ---------------------------------------------------------
#
# The Xcode Command Line Tools are the one package that covers most of what the
# rest of this script leans on: python3 (used to write Caduceus's settings), git,
# swiftc (Caduceus builds its dictation helper from Swift), and a linker for the
# Rust build. Installing them is a GUI flow Apple owns — `xcode-select --install`
# opens a window and returns immediately — so this polls rather than assuming it
# finished.

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

# --- build toolchain (--from-source only) -----------------------------------
#
# Rust and Node are needed only to compile Caduceus, which the default path does
# not do — nothing below runs unless you passed --from-source. Neither is
# installed system-wide or wired into your shell profile: Rust goes to the
# standard ~/.cargo (rustup's own home, shared with any later Rust work you do),
# Node goes under ~/.caduceus/toolchain and is put on PATH for this script alone.
# If you already have either one, yours is used untouched.

install_rust() {
  if command -v cargo >/dev/null 2>&1; then
    say "Rust is already installed ($(cargo --version 2>&1))."
    return 0
  fi
  if [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
    say "Using the Rust toolchain in ~/.cargo."
    return 0
  fi

  say "Installing Rust (~500 MB, one time)…"
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
    || die "rustup failed. Install Rust from https://rustup.rs and re-run this command."

  export PATH="$HOME/.cargo/bin:$PATH"
  command -v cargo >/dev/null 2>&1 || die "Rust installed but cargo is not on PATH."
  say "Rust installed ($(cargo --version 2>&1))."
}

install_node() {
  if command -v npm >/dev/null 2>&1; then
    say "Node is already installed ($(node --version 2>&1))."
    return 0
  fi
  if [ -x "$TOOLS_DIR/node/bin/npm" ]; then
    export PATH="$TOOLS_DIR/node/bin:$PATH"
    say "Using the Node in ${TOOLS_DIR}/node."
    return 0
  fi

  local plat
  case "$(uname -m)" in
    arm64)  plat="darwin-arm64" ;;
    x86_64) plat="darwin-x64" ;;
    *) die "Unsupported architecture: $(uname -m)" ;;
  esac

  # Ask nodejs.org which release is current LTS rather than pinning a version
  # here that goes stale. python3 is guaranteed by install_deps.
  say "Finding the current Node LTS…"
  local ver
  ver=$(curl -fsSL https://nodejs.org/dist/index.json \
    | python3 -c 'import json,sys; print(next(r["version"] for r in json.load(sys.stdin) if r["lts"]))' \
    2>/dev/null) || die "Could not reach nodejs.org. Install Node 20+ yourself and re-run this command."
  [ -n "$ver" ] || die "Could not determine the current Node LTS. Install Node 20+ yourself and re-run."

  say "Installing Node ${ver} into ${TOOLS_DIR}…"
  mkdir -p "$TOOLS_DIR/node"
  curl -fSL --progress-bar "https://nodejs.org/dist/${ver}/node-${ver}-${plat}.tar.gz" \
    | tar -xz -C "$TOOLS_DIR/node" --strip-components=1 \
    || die "Node download failed. Install Node 20+ from https://nodejs.org and re-run this command."

  export PATH="$TOOLS_DIR/node/bin:$PATH"
  command -v npm >/dev/null 2>&1 || die "Node installed but npm is not on PATH."
  say "Node installed ($(node --version 2>&1))."
}

# --- Caduceus: the download path --------------------------------------------
#
# Releases ship one universal .dmg, so there is normally nothing to choose
# between. The per-architecture names are still understood in case a future
# release splits them — but an arch we cannot match is an error rather than a
# guess, since handing an Intel Mac an arm64 build produces an app that installs
# cleanly and then refuses to open.

find_dmg() {
  local dmgs="$1" arch="$2" pick

  pick=$(echo "$dmgs" | grep -i "universal" | head -1) && [ -n "$pick" ] && { echo "$pick"; return 0; }
  pick=$(echo "$dmgs" | grep -i "$arch" | head -1) && [ -n "$pick" ] && { echo "$pick"; return 0; }

  # Nothing named for an architecture at all: a single unlabelled .dmg is the
  # ordinary single-build release, so take it. Several is ambiguous — stop.
  if [ "$(echo "$dmgs" | grep -c .)" = "1" ] && ! echo "$dmgs" | grep -qiE "aarch64|arm64|x64|x86_64|intel"; then
    echo "$dmgs"
    return 0
  fi
  return 1
}

download_caduceus() {
  local arch
  case "$(uname -m)" in
    arm64)  arch="aarch64" ;;
    x86_64) arch="x64" ;;
    *) die "Unsupported architecture: $(uname -m)" ;;
  esac

  say "Looking up the latest release…"
  local body dmgs dmg_url

  # /releases/latest is the stable channel: GitHub deliberately omits
  # prereleases from it. While Caduceus is in beta that endpoint is empty, so
  # fall back to the releases list, which is newest-first and does include
  # them. Once a stable release exists it wins, and the fallback goes quiet —
  # which is the ordering we want, not an accident of the beta.
  body=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null) || body=""

  # `|| true` matters: under `set -e` with pipefail, a grep that matches nothing
  # fails the pipeline and takes the whole script down with it, silently.
  dmgs=$(echo "$body" \
    | grep -o '"browser_download_url": *"[^"]*\.dmg"' \
    | cut -d'"' -f4 || true)

  if [ -z "$dmgs" ]; then
    say "No stable release yet — checking pre-releases…"
    # Only the newest entry: without a JSON parser, assets from several releases
    # would otherwise blur into one list and we could mix versions. Drafts are
    # invisible to an unauthenticated caller, so the first entry is the newest
    # thing a user can actually download.
    body=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases?per_page=1" 2>/dev/null) \
      || die "Could not reach the GitHub API for ${REPO}.
If you are offline or rate-limited, try again shortly — or compile it yourself:
  curl -fsSL https://vivaanshahani.com/caduceus/install.sh | bash -s -- --from-source"

    dmgs=$(echo "$body" \
      | grep -o '"browser_download_url": *"[^"]*\.dmg"' \
      | cut -d'"' -f4 || true)
  fi

  [ -n "$dmgs" ] && dmg_url=$(find_dmg "$dmgs" "$arch") || dmg_url=""

  [ -n "$dmg_url" ] || die "No .dmg for this Mac ($(uname -m)) in the latest release of ${REPO}.
Compile it yourself instead — same command, one extra flag:
  curl -fsSL https://vivaanshahani.com/caduceus/install.sh | bash -s -- --from-source"

  # Deliberately not `local`: the EXIT trap fires after this function has
  # returned, so a local here is out of scope by the time cleanup runs — which
  # under `set -u` means the script dies with "unbound variable" *after*
  # printing Done, and exits non-zero on a install that worked.
  tmp_dir=$(mktemp -d)
  mount_point="$tmp_dir/mnt"
  trap cleanup EXIT

  local dmg="$tmp_dir/caduceus.dmg"
  say "Downloading $(basename "$dmg_url")…"
  curl -fSL --progress-bar "$dmg_url" -o "$dmg"

  say "Mounting…"
  mkdir -p "$mount_point"
  hdiutil attach "$dmg" -nobrowse -quiet -mountpoint "$mount_point"

  local source_app="$mount_point/${APP_NAME}.app"
  [ -d "$source_app" ] || die "${APP_NAME}.app was not in the disk image."

  replace_installed_app "$source_app"

  # Caduceus is not notarised, so macOS would otherwise refuse to open it. This
  # is the one place the missing Developer ID costs anything, and clearing the
  # flag is exactly what the right-click-Open dance does more slowly.
  say "Clearing the quarantine flag…"
  xattr -dr com.apple.quarantine "${INSTALL_DIR}/${APP_NAME}.app" 2>/dev/null || \
    sudo xattr -dr com.apple.quarantine "${INSTALL_DIR}/${APP_NAME}.app" 2>/dev/null || \
    warn "Could not clear quarantine. Run: xattr -dr com.apple.quarantine \"${INSTALL_DIR}/${APP_NAME}.app\""
}

# --- Caduceus: the --from-source path ---------------------------------------

fetch_source() {
  if [ -d "$SRC_DIR/.git" ]; then
    say "Updating the source in ${SRC_DIR}…"
    git -C "$SRC_DIR" fetch --depth 1 origin HEAD --quiet \
      || die "Could not fetch from https://github.com/${REPO}. Check your connection and re-run."
    # Hard reset rather than pull: this checkout is ours, and a half-applied
    # merge here would surface as a confusing build error later.
    git -C "$SRC_DIR" reset --hard FETCH_HEAD --quiet
    # No -x: node_modules and the Rust target dir are ignored, and re-downloading
    # them on every run would turn a no-op update into a ten-minute one.
    git -C "$SRC_DIR" clean -fdq
  else
    say "Cloning ${REPO}…"
    rm -rf "$SRC_DIR"
    mkdir -p "$(dirname "$SRC_DIR")"
    git clone --depth 1 "https://github.com/${REPO}.git" "$SRC_DIR" --quiet \
      || die "Could not clone https://github.com/${REPO}. Check your connection and re-run."
  fi
}

build_caduceus() {
  say "Installing npm dependencies…"
  # `npm ci` is the reproducible path, but it hard-fails when the lockfile drifts
  # from package.json, which should not cost a user their install.
  ( cd "$SRC_DIR" && { npm ci --silent 2>/dev/null || npm install --silent; } ) \
    || die "npm install failed in ${SRC_DIR}."

  say "Building ${APP_NAME} — this takes a few minutes the first time…"
  # --bundles app: we copy the .app straight across, so there is no reason to
  # spend time (or hdiutil) producing a disk image nobody opens.
  ( cd "$SRC_DIR" && npm run tauri -- build --bundles app ) \
    || die "The build failed. Run it by hand for the full output:
  cd ${SRC_DIR} && npm run tauri -- build --bundles app"
}

build_and_install_caduceus() {
  # Source first: it only needs git, and knowing the current commit is what
  # tells us whether the toolchain is worth downloading at all.
  fetch_source

  local head
  head=$(git -C "$SRC_DIR" rev-parse HEAD)

  if [ "$rebuild" -eq 0 ] && [ -d "${INSTALL_DIR}/${APP_NAME}.app" ] && \
     [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$head" ]; then
    say "${APP_NAME} is already at ${head:0:7} — nothing to build. (--rebuild forces it.)"
    return 0
  fi

  install_rust
  install_node
  build_caduceus

  local built="$SRC_DIR/src-tauri/target/release/bundle/macos/${APP_NAME}.app"
  [ -d "$built" ] || die "The build finished but ${APP_NAME}.app was not at:
  ${built}"

  replace_installed_app "$built"

  mkdir -p "$CADUCEUS_HOME"
  echo "$head" > "$STAMP"
}

# --- Caduceus: shared install step -------------------------------------------

replace_installed_app() {
  local source_app="$1" target="${INSTALL_DIR}/${APP_NAME}.app"

  if [ -d "$target" ]; then
    local old_version
    old_version=$(defaults read "$target/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo "unknown")
    say "Updating ${APP_NAME} (was ${old_version}) — your settings, shortcuts, clipboard history and AI setup are kept."
    # Only the .app bundle is replaced. Everything the user has configured lives
    # in ~/Library/Application Support/${BUNDLE_ID}/ and API keys live in the
    # Keychain; neither is touched by this script, ever. That is what makes an
    # update indistinguishable from a restart.
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

  seal_signature "$target"
}

# macOS hangs privacy permissions — microphone, automation — off an app's code
# signing identity. A bundle that is only linker-signed has no usable one: its
# identifier is the executable name plus a hash, and its Info.plist is not bound
# to the signature, so TCC cannot recognise the same app twice and re-asks for
# the microphone on every single dictation.
#
# Builds now seal themselves (bundle.macOS.signingIdentity = "-"), so this is a
# no-op on anything current. It exists for the releases published before that
# fix: re-sealing in place repairs them without making anyone hunt for a newer
# download. Entitlements are not restored here and do not need to be — Caduceus
# is neither sandboxed nor hardened, so they are inert; the identifier and the
# bound Info.plist are what TCC actually keys on.
seal_signature() {
  local target="$1"

  if codesign --verify --strict "$target" >/dev/null 2>&1; then
    return 0
  fi

  say "Sealing the app signature (so macOS stops re-asking for permissions)…"
  codesign --force --sign - --identifier "$BUNDLE_ID" "$target" 2>/dev/null || \
    sudo codesign --force --sign - --identifier "$BUNDLE_ID" "$target" 2>/dev/null || \
    warn "Could not seal the signature. Caduceus still runs, but macOS may ask for
the microphone every time you dictate."
}

install_caduceus() {
  if [ "$from_source" -eq 1 ]; then
    build_and_install_caduceus
  else
    download_caduceus
  fi
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

  ${dim}Look for Caduceus in your menu bar — there is no Dock icon.${reset}

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
