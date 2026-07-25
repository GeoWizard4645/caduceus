import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import * as api from "./api";
import { cx } from "./ui";

const BRAND_PREFIX = "brand:";
const IMAGE_PREFIX = "image:";

function brandSrc(icon: string): string {
  return `/shortcut-icons/${icon.slice(BRAND_PREFIX.length)}.svg`;
}

function isEmojiLike(icon: string): boolean {
  if (!icon || icon.startsWith(BRAND_PREFIX) || icon.startsWith(IMAGE_PREFIX)) return false;
  return [...icon].length <= 2;
}

export interface ShortcutIconProps {
  icon: string;
  label: string;
  className?: string;
  imgClassName?: string;
}

/**
 * Renders a shortcut icon: bundled brand SVG, user-uploaded image, or emoji/letter.
 */
export function ShortcutIcon({ icon, label, className, imgClassName }: ShortcutIconProps) {
  const [uploadedSrc, setUploadedSrc] = useState<string | null>(null);

  useEffect(() => {
    if (!icon.startsWith(IMAGE_PREFIX)) {
      setUploadedSrc(null);
      return;
    }
    let cancelled = false;
    void api.resolveShortcutIcon(icon).then((path) => {
      if (cancelled) return;
      setUploadedSrc(path ? convertFileSrc(path) : null);
    });
    return () => {
      cancelled = true;
    };
  }, [icon]);

  if (icon.startsWith(BRAND_PREFIX)) {
    return (
      <span className={cx("inline-flex shrink-0 items-center justify-center overflow-hidden", className)}>
        <img
          src={brandSrc(icon)}
          alt=""
          className={cx("h-full w-full object-contain", imgClassName)}
          draggable={false}
        />
      </span>
    );
  }

  if (icon.startsWith(IMAGE_PREFIX) && uploadedSrc) {
    return (
      <span className={cx("inline-flex shrink-0 items-center justify-center overflow-hidden", className)}>
        <img
          src={uploadedSrc}
          alt=""
          className={cx("h-full w-full object-cover", imgClassName)}
          draggable={false}
        />
      </span>
    );
  }

  const fallback = isEmojiLike(icon) ? icon : label.charAt(0).toUpperCase();
  return (
    <span className={cx("inline-flex shrink-0 items-center justify-center leading-none", className)}>
      {fallback}
    </span>
  );
}
