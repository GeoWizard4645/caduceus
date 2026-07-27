/**
 * The features whose interaction *is* the feature.
 *
 * Most commands describe their inputs and get a form built for them (see
 * `CommandForm`). These cannot: sampling a colour off the screen, writing on a
 * sticky note, arranging files on a desktop. A form with a Run button would be
 * the wrong shape for all of them.
 *
 * Keyed by `CommandDef.page`, so the registry in `shared/commands.ts` stays
 * plain data and does not import React.
 */

import type { ComponentType } from "react";

import type { ToolPageId } from "@/shared/commands";
import type { ToolPageProps } from "../ToolPage";

import { CitationsPage } from "./CitationsPage";
import { ColorsPage } from "./ColorsPage";
import { ConvertPage } from "./ConvertPage";
import { DesktopSortPage } from "./DesktopSortPage";
import { MeetingPage } from "./MeetingPage";
import { ProcessesPage } from "./ProcessesPage";
import { ScreenRecordPage } from "./ScreenRecordPage";
import { StickyNotesPage } from "./StickyNotesPage";
import { StoragePage } from "./StoragePage";

export const TOOL_PAGES: Record<ToolPageId, ComponentType<ToolPageProps>> = {
  colors: ColorsPage,
  "sticky-notes": StickyNotesPage,
  convert: ConvertPage,
  processes: ProcessesPage,
  storage: StoragePage,
  "desktop-sort": DesktopSortPage,
  citations: CitationsPage,
  meeting: MeetingPage,
  "screen-record": ScreenRecordPage,
};
