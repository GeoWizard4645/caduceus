#!/usr/bin/env bash
#
# Cut and publish a Caduceus release.
#
#   npm run release -- patch -m "fix the thing"
#   npm run release -- 2.4.0 -m "one window, everything in tabs"
#
# Everything RELEASE.md describes by hand, in order and without the chances to
# get it wrong: bump three files that must agree, run the gates, build both
# architectures, lipo them into one universal app, sign it, pack the DMG,
# commit, tag, push, and create the GitHub release the installer reads.
#
# If CADUCEUS_SIGNING_IDENTITY is set it also hands the build to
# scripts/notarize.sh for a real Developer ID signature and an Apple ticket.
# Unset — which is the default — the release is ad-hoc signed exactly as it has
# always been. See RELEASE.md § Notarization.
#
# # The rules it enforces
#
# * **Nothing ships from a tree you have not looked at.** A dirty working tree
#   stops the release unless you pass `--all`, which commits it.
# * **Nothing ships that does not build.** The gates and the Rust tests run
#   before the version is touched, so a failure leaves the repo exactly as it
#   was rather than half-bumped.
# * **Nothing is pushed until everything local has succeeded.** The commit, the
#   tag and the push all happen after the DMG exists on disk. A build that dies
#   at 90% costs you time, not a tag pointing at nothing.
# * **It is resumable.** Re-running after a failure is safe: the version bump is
#   idempotent and an existing tag or release is reported rather than clobbered.
#
# Run `scripts/release.sh --help` for the flags.

set -euo pipefail

readonly REPO="GeoWizard4645/caduceus"
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# --- output ----------------------------------------------------------------

if [[ -t 1 ]]; then
  readonly DIM=$'\033[2m' BOLD=$'\033[1m' RED=$'\033[31m' GREEN=$'\033[32m' OFF=$'\033[0m'
else
  readonly DIM="" BOLD="" RED="" GREEN="" OFF=""
fi

step() { printf '\n%s==>%s %s%s%s\n' "$GREEN" "$OFF" "$BOLD" "$*" "$OFF"; }
note() { printf '    %s%s%s\n' "$DIM" "$*" "$OFF"; }
die()  { printf '\n%serror:%s %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }

# --- arguments -------------------------------------------------------------

usage() {
  cat <<'EOF'
Usage: npm run release -- <version|patch|minor|major> [options]

  <version>          Explicit semver, e.g. 2.4.0
  patch|minor|major  Bump the current version by that much

Options:
  -m, --message TEXT   One-line summary. Becomes the commit subject and the
                       first line of the release notes. Prompted for if absent.
  -a, --all            Commit everything in the working tree, not just the
                       version bump. Without this a dirty tree is an error.
      --notes-file F   Use F as the release notes body instead of the commit log.
      --draft          Create the GitHub release as a draft.
      --skip-tests     Skip the gates and the Rust tests. For re-runs only.
      --skip-cask      Do not update the Homebrew tap.
  -n, --dry-run        Print every step; change nothing, push nothing.
  -h, --help           This.

Examples:
  npm run release -- patch -m "fix a hotkey that could never fire"
  npm run release -- 2.4.0 -m "command pages for everything" --all
  npm run release -- minor --dry-run
EOF
}

BUMP=""
MESSAGE=""
NOTES_FILE=""
COMMIT_ALL=0
DRAFT=0
SKIP_TESTS=0
SKIP_CASK=0
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -m|--message)    MESSAGE="${2:-}"; shift 2 ;;
    --notes-file)    NOTES_FILE="${2:-}"; shift 2 ;;
    -a|--all)        COMMIT_ALL=1; shift ;;
    --draft)         DRAFT=1; shift ;;
    --skip-tests)    SKIP_TESTS=1; shift ;;
    --skip-cask)     SKIP_CASK=1; shift ;;
    -n|--dry-run)    DRY_RUN=1; shift ;;
    -h|--help)       usage; exit 0 ;;
    -*)              die "unknown option $1 (try --help)" ;;
    *)
      [[ -n "$BUMP" ]] && die "give one version, not two ($BUMP and $1)"
      BUMP="$1"; shift ;;
  esac
done

[[ -n "$BUMP" ]] || { usage; exit 1; }

# `run` is how anything with an effect is invoked, so --dry-run is honest by
# construction rather than by remembering to check a flag at each call site.
run() {
  if (( DRY_RUN )); then
    printf '    %swould run:%s %s\n' "$DIM" "$OFF" "$*"
  else
    "$@"
  fi
}

# --- work out the version --------------------------------------------------

CURRENT="$(node -p "require('./package.json').version")"

case "$BUMP" in
  major|minor|patch)
    VERSION="$(node -e '
      const [major, minor, patch] = process.argv[1].split(".").map(Number);
      const kind = process.argv[2];
      const next = kind === "major" ? [major + 1, 0, 0]
                 : kind === "minor" ? [major, minor + 1, 0]
                 : [major, minor, patch + 1];
      process.stdout.write(next.join("."));
    ' "$CURRENT" "$BUMP")"
    ;;
  *)
    [[ "$BUMP" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
      || die "\"$BUMP\" is not a semver version or one of patch/minor/major"
    VERSION="$BUMP"
    ;;
esac

readonly TAG="v${VERSION}"
readonly DMG="src-tauri/target/release/bundle/dmg/Caduceus_${VERSION}_universal.dmg"

step "Caduceus $CURRENT → $VERSION"
(( DRY_RUN )) && note "dry run: nothing will be changed, committed or pushed"

# --- preflight -------------------------------------------------------------

step "Checking the tree and the tools"

command -v gh   >/dev/null || die "the GitHub CLI is not installed — brew install gh"
command -v node >/dev/null || die "node is not installed"
command -v lipo >/dev/null || die "lipo is missing — xcode-select --install"

gh auth status >/dev/null 2>&1 || die "gh is not logged in — run: gh auth login"

rustup target list --installed | grep -qx "x86_64-apple-darwin" \
  || die "the Intel target is missing — run: rustup target add x86_64-apple-darwin"

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[[ "$BRANCH" == "main" ]] || die "on branch \"$BRANCH\" — releases are cut from main"

if [[ -n "$(git status --porcelain)" ]]; then
  if (( COMMIT_ALL )); then
    note "working tree is dirty; --all will commit all of it:"
    git status --short | sed 's/^/      /'
  else
    git status --short | sed 's/^/      /'
    die "the working tree is dirty. Commit it yourself, or re-run with --all."
  fi
fi

git fetch --quiet origin || die "could not reach origin"
if [[ -n "$(git log --oneline "HEAD..origin/$BRANCH" 2>/dev/null)" ]]; then
  die "origin/$BRANCH is ahead of you — pull first"
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  die "$TAG already exists locally. Bump to a new version, or delete it: git tag -d $TAG"
fi
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  die "$TAG is already published at github.com/$REPO/releases/tag/$TAG"
fi

note "on main, up to date with origin, $TAG is free"

# --- the summary line ------------------------------------------------------

if [[ -z "$MESSAGE" ]]; then
  if (( DRY_RUN )); then
    MESSAGE="(summary you will be prompted for)"
  else
    printf '\n    One line describing this release: '
    read -r MESSAGE
    [[ -n "$MESSAGE" ]] || die "a release needs a summary"
  fi
fi

# --- gates -----------------------------------------------------------------

if (( SKIP_TESTS )); then
  step "Skipping the gates (--skip-tests)"
else
  # Before the bump on purpose: a failure here should leave the repo untouched
  # rather than carrying a version change for a release that never happened.
  step "Running the gates"
  run npm run build
  step "Running the Rust tests"
  run npm run test:rust
fi

# --- bump ------------------------------------------------------------------

step "Setting the version in four files"

bump_json() {
  local file="$1"
  run perl -0pi -e "s/\"version\":\s*\"[^\"]+\"/\"version\": \"$VERSION\"/" "$file"
  note "$file"
}

bump_json package.json
bump_json src-tauri/tauri.conf.json

# `^version` anchors to the `[package]` key: `rust-version` does not start the
# line with `version`, and dependency versions are inside `{ … }`.
run perl -0pi -e "s/^version = \"[^\"]+\"/version = \"$VERSION\"/m" src-tauri/Cargo.toml
note "src-tauri/Cargo.toml"

# The site's SoftwareApplication schema states the shipping version, and a
# structured-data block that disagrees with the release is worse than no block
# at all — search engines read it literally.
run perl -0pi -e "s/(\"softwareVersion\": \")[^\"]+/\${1}$VERSION/" website/index.html
note "website/index.html (schema.org softwareVersion)"

# Cargo.lock carries the version too. It is not rewritten here: the builds
# below run cargo, which updates it as a side effect, and they finish before
# anything is staged. It is in the `git add` list for that reason.

if ! (( DRY_RUN )); then
  for pair in "package.json:$(node -p "require('./package.json').version")" \
              "tauri.conf.json:$(node -p "require('./src-tauri/tauri.conf.json').version")"; do
    [[ "${pair##*:}" == "$VERSION" ]] || die "${pair%%:*} is still on ${pair##*:} — the bump did not take"
  done
  grep -q "^version = \"$VERSION\"" src-tauri/Cargo.toml || die "Cargo.toml is still on the old version"
  grep -q "\"softwareVersion\": \"$VERSION\"" website/index.html \
    || die "website/index.html schema is still on the old version"
fi

# --- build -----------------------------------------------------------------

step "Building for Apple Silicon"
run npm run tauri -- build --bundles app

step "Building for Intel"
run npm run tauri -- build --target x86_64-apple-darwin --bundles app

step "Merging into one universal app"

APP_ARM="src-tauri/target/release/bundle/macos/Caduceus.app"
APP_X86="src-tauri/target/x86_64-apple-darwin/release/bundle/macos/Caduceus.app"
STAGE="src-tauri/target/release/bundle/macos/Caduceus_universal_staging"
UNIVERSAL="$STAGE/Caduceus.app"

if ! (( DRY_RUN )); then
  [[ -d "$APP_ARM" ]] || die "the Apple Silicon build is missing at $APP_ARM"
  [[ -d "$APP_X86" ]] || die "the Intel build is missing at $APP_X86"
fi

run rm -rf "$STAGE"
run mkdir -p "$STAGE"
run cp -R "$APP_ARM" "$UNIVERSAL"

# Only the main binary is fat. The Swift helpers under Contents/Resources/bin
# are host-arch; that has always been true of these releases, and they degrade
# to "helper missing" rather than crashing on the other architecture.
run lipo -create \
  "$APP_ARM/Contents/MacOS/Caduceus" \
  "$APP_X86/Contents/MacOS/Caduceus" \
  -output "$UNIVERSAL/Contents/MacOS/Caduceus"

run codesign --force --sign - --identifier com.caduceus.desktop "$UNIVERSAL"

if ! (( DRY_RUN )); then
  ARCHES="$(lipo -archs "$UNIVERSAL/Contents/MacOS/Caduceus")"
  [[ "$ARCHES" == *arm64* && "$ARCHES" == *x86_64* ]] \
    || die "the merged binary is not universal (lipo reports: $ARCHES)"
  note "universal binary: $ARCHES"
fi

step "Packing the DMG"
run mkdir -p "$(dirname "$DMG")"
run rm -f "$DMG"
run hdiutil create -volname "Caduceus" -srcfolder "$STAGE" -ov -format UDZO "$DMG" -quiet

if ! (( DRY_RUN )); then
  [[ -f "$DMG" ]] || die "the DMG was not produced at $DMG"
  note "$DMG ($(du -h "$DMG" | cut -f1))"
fi

# --- notarization ----------------------------------------------------------

# Before the commit and the push, like everything else that can fail: Apple
# rejecting the build should cost you a build, not leave a tag pointing at a
# DMG nobody can open. The script re-signs the app with the real identity,
# repacks this DMG around the stapled result, and staples that too.
if [[ -n "${CADUCEUS_SIGNING_IDENTITY:-}" ]]; then
  step "Signing and notarizing"
  run bash "$ROOT/scripts/notarize.sh" "$UNIVERSAL" "$DMG"
else
  step "Not notarizing"
  # A one-line note here used to be the only trace of this. It is easy to miss
  # between a hundred lines of build output, and the consequence — a stranger's
  # Mac calling this app malware — is not a one-line problem. See it now, not
  # when someone forwards you a screenshot of the warning.
  printf '\n    %s%s⚠ this build is ad-hoc signed, not notarized.%s\n' "$BOLD" "$RED" "$OFF"
  note "Gatekeeper will warn on it. The installer and the Homebrew cask both work"
  note "around that for people who use them (they clear the quarantine flag on"
  note "this specific, checksum-verified download). Anyone who grabs the DMG"
  note "straight off the Releases page and opens it by hand still sees:"
  note "  \"Apple could not verify ... is free of malware\""
  note "set CADUCEUS_SIGNING_IDENTITY to notarize instead — RELEASE.md § Notarization"
fi

# --- checksum -----------------------------------------------------------

# Computed after notarization, not before: notarize.sh repacks the DMG around
# the stapled app, which changes its bytes. Hashing here, once, is the only
# way the published checksum ever matches what a user actually downloads.
# website/install.sh fetches "<asset>.sha256" and refuses to clear quarantine
# on a mismatch — this is the file that makes that check possible.
step "Checksumming the DMG"
SHA_FILE="${DMG}.sha256"
if ! (( DRY_RUN )); then
  printf '%s  %s\n' "$(shasum -a 256 "$DMG" | cut -d' ' -f1)" "$(basename "$DMG")" > "$SHA_FILE"
  note "$(cat "$SHA_FILE")"
else
  note "would write: $SHA_FILE"
fi

# --- notes -----------------------------------------------------------------

LAST_TAG="$(git describe --tags --abbrev=0 2>/dev/null || true)"

if [[ -n "$NOTES_FILE" ]]; then
  [[ -f "$NOTES_FILE" ]] || die "no such notes file: $NOTES_FILE"
  BODY="$(cat "$NOTES_FILE")"
elif [[ -n "$LAST_TAG" ]]; then
  CHANGES="$(git log --no-merges --pretty='- %s' "${LAST_TAG}..HEAD" || true)"
  BODY="$MESSAGE"
  [[ -n "$CHANGES" ]] && BODY="$MESSAGE"$'\n\n## Changes\n\n'"$CHANGES"
else
  BODY="$MESSAGE"
fi

NOTES="$BODY"$'\n\n## Install\n\n```bash\ncurl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash\n```\n\n## Issues\n\nhttps://github.com/'"$REPO"$'/issues\n'

# --- commit, tag, push -----------------------------------------------------

step "Committing $VERSION"

if (( COMMIT_ALL )); then
  run git add -A
else
  run git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock \
    website/index.html
fi

if (( DRY_RUN )); then
  note "would commit: $VERSION: $MESSAGE"
elif git diff --cached --quiet; then
  note "nothing staged — the version was already committed, carrying on"
else
  git commit --quiet -m "$VERSION: $MESSAGE"
  note "$(git log -1 --oneline)"
fi

step "Tagging and pushing"
run git tag -a "$TAG" -m "$VERSION: $MESSAGE"
run git push --quiet origin "$BRANCH"
run git push --quiet origin "$TAG"
note "pushed $BRANCH and $TAG"

# --- publish ---------------------------------------------------------------

step "Creating the GitHub release"

GH_ARGS=(release create "$TAG" "$DMG" "$SHA_FILE" --repo "$REPO" --title "$TAG" --notes "$NOTES")
if (( DRAFT )); then
  GH_ARGS+=(--draft)
  note "as a draft — it will not become /releases/latest until you publish it"
else
  # What the installer resolves. Without it, curl | bash keeps serving the old
  # version and the release looks like it silently did not happen.
  GH_ARGS+=(--latest)
fi

run gh "${GH_ARGS[@]}"

# --- homebrew --------------------------------------------------------------

# After the release, not before: the cask's checksum is of the asset attached to
# it, and a tap pointing at a download that does not exist yet is worse than a
# tap that is a minute behind.
if (( SKIP_CASK )); then
  step "Skipping the Homebrew tap (--skip-cask)"
elif (( DRAFT )); then
  step "Skipping the Homebrew tap (this is a draft)"
  note "publish the draft, then: scripts/publish-cask.sh $VERSION"
elif (( DRY_RUN )); then
  step "Updating the Homebrew tap"
  note "would run: scripts/publish-cask.sh $VERSION $DMG"
else
  # A tap that fails to update is a bad afternoon, not a bad release: the DMG,
  # the tag and the installer are all already live. Report it and carry on.
  if ! bash "$ROOT/scripts/publish-cask.sh" "$VERSION" "$DMG"; then
    printf '\n%swarning:%s the Homebrew tap was not updated. The release itself is fine.\n' "$RED" "$OFF" >&2
    printf '    Retry with: scripts/publish-cask.sh %s\n' "$VERSION" >&2
  fi
fi

# --- verify ----------------------------------------------------------------

if (( DRY_RUN )); then
  step "Dry run finished — nothing was changed"
  exit 0
fi

if (( DRAFT )); then
  step "Draft ready"
  note "https://github.com/$REPO/releases/tag/$TAG"
  [[ -z "${CADUCEUS_SIGNING_IDENTITY:-}" ]] && note "unsigned — see RELEASE.md § Notarization before you publish it"
  exit 0
fi

step "Checking what the installer will now serve"

LATEST="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{
      const r = JSON.parse(s);
      process.stdout.write(`${r.tag_name} ${(r.assets||[]).map(a=>a.name).join(" ")}`);
    })' 2>/dev/null || true)"

if [[ "$LATEST" == *"$TAG"* && "$LATEST" == *universal.dmg* ]]; then
  note "/releases/latest → $LATEST"
else
  # GitHub's release API is read-through-cache; a lag of a few seconds is
  # normal and is not a failed release.
  note "/releases/latest still reads \"${LATEST:-nothing}\" — give it a moment and re-check:"
  note "curl -fsSL https://api.github.com/repos/$REPO/releases/latest | grep tag_name"
fi

step "Released $TAG"
note "https://github.com/$REPO/releases/tag/$TAG"
note "curl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash"
(( SKIP_CASK )) || note "brew install --cask geowizard4645/caduceus/caduceus"

if [[ -z "${CADUCEUS_SIGNING_IDENTITY:-}" ]]; then
  printf '\n%s%s⚠ unsigned release.%s Gatekeeper will call this app unverified for anyone\n' "$BOLD" "$RED" "$OFF"
  printf '  who does not install it through the two paths above. That is a Developer ID\n'
  printf '  and $99/year away from going away entirely — see RELEASE.md %s§ Notarization%s.\n' "$DIM" "$OFF"
fi
