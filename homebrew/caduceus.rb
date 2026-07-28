# Caduceus, as a Homebrew cask.
#
# This file is the source of truth. It is copied to `Casks/caduceus.rb` in the
# tap repo (GeoWizard4645/homebrew-caduceus) by `scripts/publish-cask.sh`, which
# `npm run release` runs for you — so the version and checksum below are
# rewritten on every release and should not be edited by hand.
#
#   brew install --cask geowizard4645/caduceus/caduceus
#
# Why a tap of our own rather than homebrew-cask: the official repository has a
# notability bar (a project has to be established before they will carry it) and
# a review queue measured in days. A tap is the same command for the user, ships
# the moment the release does, and can move to homebrew-cask later without
# anybody having to change how they installed it.

cask "caduceus" do
  version "3.3.2"
  sha256 "11c4cbe943906282332f58a0cab68895de932dfd11c8a6f53d72ac2c6059521a"

  url "https://github.com/GeoWizard4645/caduceus/releases/download/v#{version}/Caduceus_#{version}_universal.dmg",
      verified: "github.com/GeoWizard4645/caduceus/"
  name "Caduceus"
  desc "Local-first command center: launcher, clipboard, dictation and AI agents"
  homepage "https://caduceus.vivaanshahani.com/"

  livecheck do
    url :url
    strategy :github_latest
  end

  # Caduceus has no self-updater; `brew upgrade` is how it moves.
  auto_updates false
  # Big Sur and up, matching `bundle.macOS.minimumSystemVersion` in tauri.conf.json.
  depends_on macos: :big_sur

  app "Caduceus.app"

  # Caduceus is not notarised — there is no Apple Developer certificate behind
  # it — so macOS quarantines the app and refuses to open it. Homebrew carries
  # that flag through from the download, which would leave every cask install
  # face-first into "Caduceus is damaged and can't be opened".
  #
  # Clearing the flag is exactly what right-click → Open does, only without
  # making you find out that is what you needed to do. It is the same line the
  # curl installer runs, and it is here in the open rather than in a script you
  # did not read. If you would rather decide for yourself, install with
  # `--no-quarantine` and this becomes a no-op, or leave the flag on and approve
  # the app in System Settings → Privacy & Security the first time.
  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Caduceus.app"],
                   sudo: false
  end

  uninstall quit: "com.caduceus.desktop"

  # Everything Caduceus writes, and nothing it does not. `brew uninstall` leaves
  # all of this alone; `brew uninstall --zap` is the one that means "and my
  # settings too". API keys live in the login Keychain and are deliberately not
  # listed — no uninstaller should be reaching in there.
  zap trash: [
    "~/.caduceus",
    "~/Library/Application Support/com.caduceus.desktop",
    "~/Library/Caches/com.caduceus.desktop",
    "~/Library/HTTPStorages/com.caduceus.desktop",
    "~/Library/Preferences/com.caduceus.desktop.plist",
    "~/Library/Saved Application State/com.caduceus.desktop.savedState",
    "~/Library/WebKit/com.caduceus.desktop",
  ]

  caveats <<~EOS
    Caduceus lives in the menu bar — there is no Dock icon.

      Control+Space   open the Command Center
      F12             hide or show the floating staff
      Alt+Shift+V     hold to talk

    Window snapping and reading the selected text need Accessibility, and
    dictation needs the Microphone. Caduceus asks the first time you use them
    and walks you through granting it.
  EOS
end
