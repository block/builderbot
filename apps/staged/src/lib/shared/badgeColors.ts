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

/** Slider track color: the badge hue at mid lightness, between badgeBg
 *  (too pale to tell hues apart) and badgeFg (too dark to read as the
 *  badge tint), with enough chroma to keep hues distinguishable. */
function hueSliderStop(hue: number, dark: boolean): string {
  return dark ? `oklch(0.55 0.10 ${hue})` : `oklch(0.80 0.10 ${hue})`;
}

/**
 * Track gradient for the badge hue slider, sampled every 15° in OKLCH
 * so the color under the thumb matches the badge the chosen hue
 * actually produces (HSL hue angles don't line up with OKLCH hue
 * angles).
 */
export function hueSliderGradient(dark: boolean): string {
  const stops: string[] = [];
  for (let hue = 0; hue <= 360; hue += 15) {
    stops.push(hueSliderStop(hue, dark));
  }
  return `linear-gradient(to right, ${stops.join(', ')})`;
}
