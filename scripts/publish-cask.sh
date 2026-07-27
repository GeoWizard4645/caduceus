#!/usr/bin/env bash
#
# Push the Homebrew cask for a release.
#
#   scripts/publish-cask.sh 2.4.0 [path/to/Caduceus_2.4.0_universal.dmg]
#
# `npm run release` calls this at the end, so a normal release keeps the tap in
# step by itself. Run it by hand to repair a tap that drifted, or to publish a
# cask for a release that was cut before this existed.
#
# # What it does
#
# Rewrites the version and checksum in `homebrew/caduceus.rb` — the copy in this
# repo is the source of truth — and copies it into `Casks/caduceus.rb` in the
# tap repo, which is a second, tiny repository that exists only to be tapped.
#
# The checksum comes from the DMG on disk if you pass one, and from the release
# on GitHub otherwise. The second is the more honest check of the two: it
# verifies the bytes a user will actually download rather than the ones that
# happened to be in your build directory.
#
# # The tap
#
# Homebrew maps `brew tap owner/name` to `github.com/owner/homebrew-name`, which
# is why the repository is called homebrew-caduceus and the install line reads
#
#   brew install --cask geowizard4645/caduceus/caduceus
#
# The first run offers to create that repository. Nothing is created without
# being asked.

set -euo pipefail

readonly REPO="GeoWizard4645/caduceus"
readonly TAP_REPO="GeoWizard4645/homebrew-caduceus"
readonly TAP_NAME="geowizard4645/caduceus"
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -t 1 ]]; then
  readonly DIM=$'\033[2m' BOLD=$'\033[1m' RED=$'\033[31m' GREEN=$'\033[32m' OFF=$'\033[0m'
else
  readonly DIM="" BOLD="" RED="" GREEN="" OFF=""
fi

step() { printf '\n%s==>%s %s%s%s\n' "$GREEN" "$OFF" "$BOLD" "$*" "$OFF"; }
note() { printf '    %s%s%s\n' "$DIM" "$*" "$OFF"; }
warn() { printf '    %swarning:%s %s\n' "$RED" "$OFF" "$*" >&2; }
die()  { printf '\n%serror:%s %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }

VERSION="${1:-}"
DMG="${2:-}"
[[ -n "$VERSION" ]] || die "usage: scripts/publish-cask.sh <version> [dmg]"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "\"$VERSION\" is not a semver version"

readonly TAG="v${VERSION}"
readonly ASSET="Caduceus_${VERSION}_universal.dmg"
readonly CASK_SRC="$ROOT/homebrew/caduceus.rb"

command -v gh >/dev/null || die "the GitHub CLI is not installed — brew install gh"
gh auth status >/dev/null 2>&1 || die "gh is not logged in — run: gh auth login"
[[ -f "$CASK_SRC" ]] || die "no cask at $CASK_SRC"

# --- the tap repository ----------------------------------------------------

step "Checking the tap"

if ! gh repo view "$TAP_REPO" >/dev/null 2>&1; then
  printf '\n    %s%s does not exist yet.%s\n' "$BOLD" "$TAP_REPO" "$OFF"
  note "It is the repository Homebrew reads for \`brew install --cask $TAP_NAME/caduceus\`."
  note "It holds one file and nothing else."

  if [[ ! -t 0 ]]; then
    warn "not running interactively, so it will not be created."
    note "Create it yourself with: gh repo create $TAP_REPO --public"
    exit 1
  fi

  printf '\n    Create it now, public? [y/N] '
  read -r reply
  [[ "$reply" =~ ^[Yy]$ ]] || die "stopped. Nothing was created."

  gh repo create "$TAP_REPO" --public \
    --description "Homebrew tap for Caduceus — brew install --cask $TAP_NAME/caduceus" \
    >/dev/null
  note "created github.com/$TAP_REPO"
fi

# --- the checksum ----------------------------------------------------------

step "Checksumming $ASSET"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [[ -n "$DMG" ]]; then
  [[ -f "$DMG" ]] || die "no such file: $DMG"
  note "from the local build"
else
  # The bytes a user will actually get, rather than the ones in the build
  # directory — which is the whole point of a checksum.
  DMG="$WORK/$ASSET"
  note "downloading from the $TAG release"
  gh release download "$TAG" --repo "$REPO" --pattern "$ASSET" --dir "$WORK" \
    || die "$ASSET is not attached to the $TAG release of $REPO"
fi

SHA="$(shasum -a 256 "$DMG" | cut -d' ' -f1)"
note "sha256 $SHA"

# --- render ----------------------------------------------------------------

step "Updating homebrew/caduceus.rb"

perl -0pi -e "s/^  version \"[^\"]+\"/  version \"$VERSION\"/m" "$CASK_SRC"
perl -0pi -e "s/^  sha256 \"[^\"]+\"/  sha256 \"$SHA\"/m" "$CASK_SRC"

grep -q "  version \"$VERSION\"" "$CASK_SRC" || die "the version did not take in $CASK_SRC"
grep -q "  sha256 \"$SHA\"" "$CASK_SRC"      || die "the checksum did not take in $CASK_SRC"

# `brew audit` needs the cask inside a tap, so it runs after the push. `ruby -c`
# is what catches the mistake that matters here — a cask that will not parse
# breaks `brew update` for everyone who has tapped it, not just installers.
if command -v ruby >/dev/null; then
  ruby -c "$CASK_SRC" >/dev/null || die "$CASK_SRC is not valid Ruby"
  note "parses"
fi

# --- publish ---------------------------------------------------------------

step "Pushing to $TAP_REPO"

CLONE="$WORK/tap"
gh repo clone "$TAP_REPO" "$CLONE" -- --quiet 2>/dev/null || die "could not clone $TAP_REPO"

mkdir -p "$CLONE/Casks"
cp "$CASK_SRC" "$CLONE/Casks/caduceus.rb"

# A tap's README is the page people land on from `brew info`, so it should say
# what to type. Written once and then left alone.
if [[ ! -f "$CLONE/README.md" ]]; then
  cat > "$CLONE/README.md" <<EOF
# Caduceus — Homebrew tap

A fast, local-first command center for macOS: launcher, clipboard history,
dictation, window management and optional local AI.

\`\`\`bash
brew install --cask $TAP_NAME/caduceus
\`\`\`

To update: \`brew upgrade --cask caduceus\`.
To remove it and everything it stored: \`brew uninstall --zap --cask caduceus\`.

Caduceus is not notarised, so the cask clears the quarantine flag after
installing — the same thing right-click → Open does. Pass \`--no-quarantine\`
to skip that and approve it yourself in System Settings → Privacy & Security.

The cask is generated from [\`homebrew/caduceus.rb\`](https://github.com/$REPO/blob/main/homebrew/caduceus.rb)
in the [main repository](https://github.com/$REPO); edit it there.
EOF
fi

cd "$CLONE"
git add Casks/caduceus.rb README.md

if git diff --cached --quiet; then
  note "the tap is already on $VERSION — nothing to push"
else
  git commit --quiet -m "caduceus $VERSION"
  git push --quiet origin HEAD
  note "pushed caduceus $VERSION"
fi

cd "$ROOT"

# --- verify ----------------------------------------------------------------

if command -v brew >/dev/null; then
  step "Checking Homebrew can read it"
  # `--force-auto-update` keeps a tap that is already present from going stale
  # and reporting the previous version back at us.
  brew tap --force-auto-update "$TAP_NAME" >/dev/null 2>&1 || true
  brew update --quiet >/dev/null 2>&1 || true

  FOUND="$(brew info --cask "$TAP_NAME/caduceus" 2>/dev/null | head -1 || true)"
  if [[ "$FOUND" == *"$VERSION"* ]]; then
    note "$FOUND"
  else
    note "brew reports \"${FOUND:-nothing}\" — GitHub's raw cache lags a minute or two"
  fi

  brew audit --cask --online --new "$TAP_NAME/caduceus" 2>&1 | sed 's/^/    /' || \
    note "audit had findings (above). A tap is not held to homebrew-cask's rules, so read them rather than obeying them."
fi

step "Cask published"
note "brew install --cask $TAP_NAME/caduceus"
