/**
 * Centralized OKLCH badge color helpers.
 *
 * All badge/chip color construction flows through here so the
 * lightness and chroma values are defined in exactly one place.
 */

export function badgeBg(hue: number, dark: boolean): string {
  return dark ? `oklch(0.30 0.04 ${hue})` : `oklch(0.95 0.03 ${hue})`;
}

export function badgeFg(hue: number, dark: boolean): string {
  return dark ? `oklch(0.78 0.10 ${hue})` : `oklch(0.45 0.12 ${hue})`;
}

export function badgeBgHover(hue: number, dark: boolean): string {
  return dark ? `oklch(0.35 0.05 ${hue})` : `oklch(0.92 0.04 ${hue})`;
}

export function badgeBorder(hue: number, dark: boolean): string {
  return dark ? `oklch(0.38 0.05 ${hue})` : `oklch(0.87 0.05 ${hue})`;
}

export function badgeBorderHover(hue: number, dark: boolean): string {
  return dark ? `oklch(0.50 0.08 ${hue})` : `oklch(0.72 0.09 ${hue})`;
}

export function badgeShortcutBg(hue: number, dark: boolean): string {
  return dark ? `oklch(0.40 0.06 ${hue})` : `oklch(0.86 0.06 ${hue})`;
}
