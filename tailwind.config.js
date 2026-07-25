/** @type {import('tailwindcss').Config} */

// Every colour resolves through a CSS custom property so that the theme (dark /
// light) and the accent colour can be swapped at runtime from the Appearance
// settings tab without a rebuild. See src/shared/theme.ts and src/styles.css.
const cssVar = (name) => `rgb(var(${name}) / <alpha-value>)`;

export default {
  content: ["./index.html", "./command-center.html", "./settings.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // Backdrop layers, from furthest back to closest to the user.
        base: cssVar("--c-base"),
        surface: cssVar("--c-surface"),
        raised: cssVar("--c-raised"),
        overlay: cssVar("--c-overlay"),

        // Hairlines and separators.
        line: cssVar("--c-line"),
        "line-strong": cssVar("--c-line-strong"),

        // Type.
        ink: cssVar("--c-ink"),
        "ink-soft": cssVar("--c-ink-soft"),
        "ink-mute": cssVar("--c-ink-mute"),
        "ink-faint": cssVar("--c-ink-faint"),

        // The single confident accent, plus semantic states.
        accent: cssVar("--c-accent"),
        "accent-soft": cssVar("--c-accent-soft"),
        "accent-ink": cssVar("--c-accent-ink"),
        positive: cssVar("--c-positive"),
        caution: cssVar("--c-caution"),
        danger: cssVar("--c-danger"),
      },
      fontFamily: {
        sans: [
          "Inter var",
          "Inter",
          "-apple-system",
          "BlinkMacSystemFont",
          "SF Pro Text",
          "Segoe UI",
          "system-ui",
          "sans-serif",
        ],
        mono: ["ui-monospace", "SF Mono", "JetBrains Mono", "Menlo", "Consolas", "monospace"],
      },
      fontSize: {
        "2xs": ["0.6875rem", { lineHeight: "1rem", letterSpacing: "0.01em" }],
      },
      borderRadius: {
        cad: "18px",
        "cad-lg": "24px",
      },
      boxShadow: {
        // Depth is carried by layered, low-opacity shadows rather than borders.
        float: "0 1px 2px rgb(0 0 0 / 0.28), 0 8px 24px -6px rgb(0 0 0 / 0.36), 0 24px 64px -12px rgb(0 0 0 / 0.45)",
        panel: "0 1px 1px rgb(0 0 0 / 0.20), 0 12px 40px -8px rgb(0 0 0 / 0.42)",
        "inner-hair": "inset 0 1px 0 0 rgb(255 255 255 / 0.06)",
        glow: "0 0 0 1px rgb(var(--c-accent) / 0.35), 0 0 24px -4px rgb(var(--c-accent) / 0.45)",
      },
      backdropBlur: {
        glass: "32px",
      },
      transitionTimingFunction: {
        // A single spring-ish curve used everywhere so motion feels coherent.
        cad: "cubic-bezier(0.22, 1, 0.36, 1)",
        snap: "cubic-bezier(0.32, 0.72, 0, 1)",
      },
      keyframes: {
        "fade-rise": {
          from: { opacity: "0", transform: "translateY(6px) scale(0.985)" },
          to: { opacity: "1", transform: "translateY(0) scale(1)" },
        },
        "staff-pulse": {
          "0%, 100%": { transform: "scale(1)", opacity: "0.85" },
          "50%": { transform: "scale(1.06)", opacity: "1" },
        },
        "spin-slow": {
          from: { transform: "rotate(0deg)" },
          to: { transform: "rotate(360deg)" },
        },
        shimmer: {
          from: { backgroundPosition: "200% 0" },
          to: { backgroundPosition: "-200% 0" },
        },
      },
      animation: {
        "fade-rise": "fade-rise 180ms cubic-bezier(0.22, 1, 0.36, 1) both",
        "staff-pulse": "staff-pulse 5.5s cubic-bezier(0.4, 0, 0.6, 1) infinite",
        "spin-slow": "spin-slow 14s linear infinite",
        shimmer: "shimmer 2s linear infinite",
      },
    },
  },
  plugins: [],
};
