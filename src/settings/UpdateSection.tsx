import * as api from "@/shared/api";
import { useUpdateCheck } from "@/shared/hooks";
import { Button, Callout, Section } from "@/shared/ui";

export function UpdateSection() {
  const update = useUpdateCheck(true);

  const openUpdate = () => {
    const url =
      update?.downloadUrl ??
      update?.releaseUrl ??
      "https://github.com/GeoWizard4645/caduceus/releases/latest";
    void api.openExternalUrl(url);
  };

  if (!update?.updateAvailable) {
    return null;
  }

  return (
    <Section
      title="Update available"
      description={`Caduceus ${update.latestVersion ?? ""} is on GitHub — you are on ${update.currentVersion}.`}
    >
      <Callout tone="info">
        <p className="text-[13px] leading-relaxed text-ink-soft">
          Download the new universal .dmg and replace the app in Applications, or run{" "}
          <code className="text-ink">brew upgrade --cask caduceus</code> if you installed with Homebrew.
        </p>
        <div className="row mt-3 gap-2">
          <Button tone="primary" size="sm" onClick={openUpdate}>
            Get update
          </Button>
          {update.releaseUrl && (
            <Button size="sm" onClick={() => void api.openExternalUrl(update.releaseUrl!)}>
              Release notes
            </Button>
          )}
        </div>
      </Callout>
    </Section>
  );
}
