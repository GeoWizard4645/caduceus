import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import * as api from "./api";
import { useTauriEvent } from "./hooks";
import { CaduceusMark } from "./CaduceusMark";
import { EVENTS } from "./types";
import { cx } from "./ui";

const IMAGE_PREFIX = "image:";

export function StaffMark({
  height,
  icon,
  className,
  title = "Caduceus staff",
}: {
  height: number;
  /** Empty string = built-in caduceus pixels. */
  icon: string;
  className?: string;
  title?: string;
}) {
  const [uploadedSrc, setUploadedSrc] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);

  useTauriEvent(EVENTS.staffMarkChanged, () => {
    setRevision((r) => r + 1);
  });

  useEffect(() => {
    if (!icon.startsWith(IMAGE_PREFIX)) {
      setUploadedSrc(null);
      return;
    }
    let cancelled = false;
    void api.resolveStaffMark(icon).then((path) => {
      if (cancelled) return;
      setUploadedSrc(
        path ? `${convertFileSrc(path)}?v=${revision}-${Date.now()}` : null,
      );
    });
    return () => {
      cancelled = true;
    };
  }, [icon, revision]);

  if (icon.startsWith(IMAGE_PREFIX) && uploadedSrc) {
    const width = Math.round(height * 0.72);
    return (
      <img
        src={uploadedSrc}
        alt=""
        width={width}
        height={height}
        draggable={false}
        className={cx("object-contain", className)}
        style={{ imageRendering: "pixelated" }}
        role="img"
        aria-label={title}
      />
    );
  }

  return <CaduceusMark height={height} className={className} title={title} />;
}
