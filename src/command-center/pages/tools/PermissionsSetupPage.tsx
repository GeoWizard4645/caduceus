/**
 * Check and fix Caduceus privacy grants — replaces the old text-only output.
 */

import { useEffect } from "react";

import { Section } from "@/shared/ui";

import { PermissionSetupPanel } from "../PermissionSetupPanel";
import type { ToolPageProps } from "../ToolPage";

export function PermissionsSetupPage({ active, onOpenTab, onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Permissions"), [onSetTitle]);

  return (
    <div className="mx-auto max-w-[640px] px-6 py-5">
      <Section
        title="Permissions"
        description="What Caduceus needs from macOS. Set up opens the system prompt and the right Settings pane; Repair fixes grants that break after an update."
      >
        <PermissionSetupPanel active={active} onOpenTab={onOpenTab} />
      </Section>
    </div>
  );
}
