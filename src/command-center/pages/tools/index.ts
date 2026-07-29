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

import { BirthdaysPage } from "./BirthdaysPage";
import { CitationsPage } from "./CitationsPage";
import { ColorsPage } from "./ColorsPage";
import { ConvertPage } from "./ConvertPage";
import { CronPage } from "./CronPage";
import { CurlPage } from "./CurlPage";
import { DependenciesPage } from "./DependenciesPage";
import { DesktopShapesPage } from "./DesktopShapesPage";
import { DesktopSortPage } from "./DesktopSortPage";
import { DocumentsPage } from "./DocumentsPage";
import { GitCommitPage } from "./GitCommitPage";
import { HabitsPage } from "./HabitsPage";
import { ImagesPage } from "./ImagesPage";
import { MeetingPage } from "./MeetingPage";
import { PermissionsSetupPage } from "./PermissionsSetupPage";
import { ProcessesPage } from "./ProcessesPage";
import { PromptOptimizerPage } from "./PromptOptimizerPage";
import { QrPage } from "./QrPage";
import { RegexPage } from "./RegexPage";
import { ScreenRecordPage } from "./ScreenRecordPage";
import { SearchPage } from "./SearchPage";
import { SecurityPage } from "./SecurityPage";
import { SnippetsPage } from "./SnippetsPage";
import { StickyNotesPage } from "./StickyNotesPage";
import { StoragePage } from "./StoragePage";
import { SubscriptionsPage } from "./SubscriptionsPage";
import { TimePage } from "./TimePage";
import { TotpPage } from "./TotpPage";
import { VoiceTypePage } from "./VoiceTypePage";
import { WallpaperPage } from "./WallpaperPage";
import { WidgetsPage } from "./WidgetsPage";

export const TOOL_PAGES: Record<ToolPageId, ComponentType<ToolPageProps>> = {
  colors: ColorsPage,
  qr: QrPage,
  "sticky-notes": StickyNotesPage,
  convert: ConvertPage,
  processes: ProcessesPage,
  storage: StoragePage,
  "desktop-sort": DesktopSortPage,
  "desktop-shapes": DesktopShapesPage,
  citations: CitationsPage,
  meeting: MeetingPage,
  "voice-type": VoiceTypePage,
  permissions: PermissionsSetupPage,
  "screen-record": ScreenRecordPage,
  security: SecurityPage,
  time: TimePage,
  regex: RegexPage,
  "prompt-optimizer": PromptOptimizerPage,
  cron: CronPage,
  images: ImagesPage,
  search: SearchPage,
  documents: DocumentsPage,
  snippets: SnippetsPage,
  widgets: WidgetsPage,
  curl: CurlPage,
  "git-commit": GitCommitPage,
  dependencies: DependenciesPage,
  habits: HabitsPage,
  birthdays: BirthdaysPage,
  subscriptions: SubscriptionsPage,
  totp: TotpPage,
  wallpaper: WallpaperPage,
};
