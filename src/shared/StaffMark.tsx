import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import * as api from "./api";
import { CaduceusMark } from "./CaduceusMark";
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

  useEffect(() => {
    if (!icon.startsWith(IMAGE_PREFIX)) {
      setUploadedSrc(null);
      return;
    }
    let cancelled = false;
    void api.resolveStaffMark(icon).then((path) => {
      if (cancelled) return;
      setUploadedSrc(path ? convertFileSrc(path) : null);
    });
    return () => {
      cancelled = true;
    };
  }, [icon]);

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
