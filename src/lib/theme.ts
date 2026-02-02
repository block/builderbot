/**
 * Adaptive Theme Infrastructure for Staged
 *
 * The UI chrome colors are derived from the syntax highlighting theme.
 * This ensures a unified look where the sidebar and controls blend
 * seamlessly with the code area.
 *
 * Key insight: we detect light vs dark themes and adjust our derivation
 * strategy accordingly. Light themes need darkening, dark themes need lightening.
 *
 * Usage in CSS (via CSS custom properties):
 *   color: var(--text-primary);
 *   background: var(--bg-primary);
 */

export interface Theme {
  // Is this a light or dark theme?
  isDark: boolean;

  // Base colors
  bg: {
    primary: string; // Main background (same as syntax theme) - used for editor islands
    chrome: string; // Unified chrome background (header, sidebar, spine)
    deepest: string; // Deepest level (tab bar) - pure black/white
    elevated: string; // Floating elements (dropdowns, tooltips)
    hover: string; // Hover states
  };

  // Borders and dividers
  border: {
    subtle: string; // Very subtle dividers
    muted: string; // Standard borders
    emphasis: string; // Emphasized borders (focus, range markers)
  };

  // Text colors
  text: {
    primary: string; // Main text
    muted: string; // Subdued text (from comment color)
    faint: string; // Very subdued (placeholders)
    accent: string; // Links and interactive text
  };

  // Git status colors (adapted for light/dark)
  status: {
    modified: string;
    added: string;
    deleted: string;
    renamed: string;
    untracked: string;
  };

  // Diff viewer colors
  diff: {
    addedBg: string; // Background tint for added lines
    removedBg: string; // Background tint for removed lines
    changedBg: string; // Neutral background for changed lines (not add/remove specific)
    rangeBorder: string; // Border color for change range markers
    commentHighlight: string; // Highlight color for commented regions in spine
  };

  // Search highlight colors
  search: {
    matchBg: string; // Background for search matches
    currentMatchBg: string; // Background for current/focused match
  };

  // AI annotation overlays
  annotation: {
    overlayBg: string; // Semi-transparent background for blur overlay
    border: string; // Subtle border for overlay edges
  };

  // Interactive elements
  ui: {
    accent: string; // Primary accent
    accentHover: string; // Accent hover
    danger: string; // Destructive actions
    dangerBg: string; // Danger background (for error messages)
    selection: string; // Selected items background (theme-derived)
  };

  // Scrollbar
  scrollbar: {
    thumb: string;
    thumbHover: string;
    thumbTransparent: string;
    thumbHoverTransparent: string;
  };

  // Shadows and overlays (for modals, dropdowns, etc.)
  shadow: {
    overlay: string; // Modal backdrop
    elevated: string; // Floating element shadows
    glow: string; // Selection glow effect
  };
}

// =============================================================================
// Color Utilities
// =============================================================================

interface RGB {
  r: number;
  g: number;
  b: number;
}

function hexToRgb(hex: string): RGB {
  // Try 6-digit (#rrggbb) or 8-digit (#rrggbbaa) hex
  const long = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})?$/i.exec(hex);
  if (long) {
    return {
      r: parseInt(long[1], 16),
      g: parseInt(long[2], 16),
      b: parseInt(long[3], 16),
    };
  }

  // Try 3-digit (#rgb) or 4-digit (#rgba) shorthand
  const short = /^#?([a-f\d])([a-f\d])([a-f\d])([a-f\d])?$/i.exec(hex);
  if (short) {
    return {
      r: parseInt(short[1] + short[1], 16),
      g: parseInt(short[2] + short[2], 16),
      b: parseInt(short[3] + short[3], 16),
    };
  }

  return { r: 128, g: 128, b: 128 };
}

function rgbToHex({ r, g, b }: RGB): string {
  const clamp = (n: number) => Math.max(0, Math.min(255, Math.round(n)));
  return `#${[r, g, b].map((c) => clamp(c).toString(16).padStart(2, '0')).join('')}`;
}

/**
 * Calculate relative luminance (0-1) for a color.
 * Used to determine if a theme is light or dark.
 */
function luminance(hex: string): number {
  const { r, g, b } = hexToRgb(hex);
  const [rs, gs, bs] = [r, g, b].map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * rs + 0.7152 * gs + 0.0722 * bs;
}

/**
 * Mix two colors by a factor (0 = color1, 1 = color2)
 */
function mix(hex1: string, hex2: string, factor: number): string {
  const c1 = hexToRgb(hex1);
  const c2 = hexToRgb(hex2);
  return rgbToHex({
    r: c1.r + (c2.r - c1.r) * factor,
    g: c1.g + (c2.g - c1.g) * factor,
    b: c1.b + (c2.b - c1.b) * factor,
  });
}

/**
 * Adjust a color toward white (positive) or black (negative)
 */
function adjust(hex: string, amount: number): string {
  const target = amount > 0 ? '#ffffff' : '#000000';
  return mix(hex, target, Math.abs(amount));
}

/**
 * Create a semi-transparent overlay color
 */
function overlay(hex: string, alpha: number): string {
  const { r, g, b } = hexToRgb(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

// =============================================================================
// Adaptive Theme Generator
// =============================================================================

/**
 * Result of chrome/primary color calculation
 */
interface ChromeColors {
  chrome: string;
  deepest: string;
  primary: string; // May be adjusted from original if needed for contrast
}

/**
 * Binary search to find a color with target luminance by mixing toward black or white.
 */
function findColorWithLuminance(baseColor: string, targetLum: number): string {
  const baseLum = luminance(baseColor);
  if (Math.abs(baseLum - targetLum) < 0.001) return baseColor;

  const target = targetLum < baseLum ? '#000000' : '#ffffff';
  let lo = 0,
    hi = 1;

  for (let i = 0; i < 20; i++) {
    const mid = (lo + hi) / 2;
    const testLum = luminance(mix(baseColor, target, mid));
    const diff = testLum - targetLum;

    if (Math.abs(diff) < 0.001) break;

    if (target === '#000000') {
      if (testLum > targetLum) lo = mid;
      else hi = mid;
    } else {
      if (testLum < targetLum) lo = mid;
      else hi = mid;
    }
  }
  return mix(baseColor, target, (lo + hi) / 2);
}

// =============================================================================
// Chrome/Primary Contrast Calculation
// =============================================================================

// Tuned values for logFloor algorithm
// Formula: diff = value * ln(1 + (lum + offset) * 10)
// - offset provides a floor so very dark themes still get some contrast
// - value scales the overall contrast
const CONTRAST_VALUE = 0.035;
const CONTRAST_OFFSET = 0.0135;

/**
 * Calculate target luminance difference using logFloor algorithm.
 * This provides gentle scaling that works across all theme luminances,
 * with a floor for very dark themes.
 */
function calculateLumDiff(bgLum: number): number {
  return CONTRAST_VALUE * Math.log(1 + (bgLum + CONTRAST_OFFSET) * 10);
}

/**
 * Calculate chrome and primary background colors.
 *
 * Strategy:
 * 1. Calculate luminance difference using logFloor algorithm
 * 2. Darken chrome to achieve that difference
 * 3. If theme is too dark, set chrome to black and lighten primary instead
 */
function calculateChromeColors(syntaxBg: string): ChromeColors {
  const bgLum = luminance(syntaxBg);

  // Calculate target luminance difference for chrome
  const lumDiff = calculateLumDiff(bgLum);
  const targetChromeLum = bgLum - lumDiff;

  // Deepest is 2x the chrome difference (darker still)
  const deepestLumDiff = lumDiff * 2;
  const targetDeepestLum = bgLum - deepestLumDiff;

  // Can we achieve chrome luminance? (must be >= 0)
  if (targetChromeLum >= 0) {
    return {
      chrome: findColorWithLuminance(syntaxBg, targetChromeLum),
      // Deepest goes as dark as possible, clamped to 0
      deepest: findColorWithLuminance(syntaxBg, Math.max(0, targetDeepestLum)),
      primary: syntaxBg,
    };
  }

  // Theme is too dark - chrome goes black, lighten primary
  // Deepest also goes black (can't go darker than 0 luminance)
  return {
    chrome: findColorWithLuminance(syntaxBg, 0),
    deepest: findColorWithLuminance(syntaxBg, 0),
    primary: findColorWithLuminance(syntaxBg, lumDiff),
  };
}

/**
 * Git colors from the syntax theme (optional - may be null if theme doesn't define them)
 */
export interface ThemeGitColors {
  added: string | null;
  deleted: string | null;
  modified: string | null;
}

/**
 * Create an adaptive theme based on syntax theme colors.
 *
 * The key insight is detecting light vs dark and adjusting accordingly:
 * - Dark themes: lighten to create elevation/emphasis
 * - Light themes: darken to create elevation/emphasis
 *
 * Git colors are taken from the theme when available, with fallbacks for themes
 * that don't define them.
 */
export function createAdaptiveTheme(
  syntaxBg: string,
  syntaxFg: string,
  syntaxComment: string,
  gitColors?: ThemeGitColors
): Theme {
  const isDark = luminance(syntaxBg) < 0.5;

  // Calculate chrome, deepest, and potentially adjusted primary
  const {
    chrome: chromeColor,
    deepest: deepestColor,
    primary: primaryBg,
  } = calculateChromeColors(syntaxBg);

  // Direction multiplier: +1 for dark (lighten), -1 for light (darken)
  const dir = isDark ? 1 : -1;

  // Base adjustments - smaller values for subtlety
  // Use the (potentially adjusted) primary bg as the base
  const elevate = (amount: number) => adjust(primaryBg, dir * amount);

  // Fallback accent colors (used when theme doesn't provide git colors)
  const fallbackBlue = isDark ? '#58a6ff' : '#0969da';
  const fallbackGreen = isDark ? '#3fb950' : '#1a7f37';
  const fallbackRed = isDark ? '#f85149' : '#cf222e';
  const fallbackOrange = isDark ? '#d29922' : '#9a6700';

  // Use theme git colors when available, fallback otherwise
  const accentGreen = gitColors?.added ?? fallbackGreen;
  const accentRed = gitColors?.deleted ?? fallbackRed;
  const accentBlue = gitColors?.modified ?? fallbackBlue;
  const accentOrange = fallbackOrange; // Used for warnings/caution UI elements

  // Border that's visible but not harsh
  const borderBase = mix(primaryBg, syntaxFg, isDark ? 0.15 : 0.12);

  return {
    isDark,

    bg: {
      primary: primaryBg, // Editor islands - may be adjusted from syntax theme for contrast
      chrome: chromeColor, // Calculated for consistent contrast ratio
      deepest: deepestColor, // Darker than chrome (2x the luminance diff)
      elevated: elevate(0.08), // Floating elements (dropdowns, tooltips)
      hover: elevate(0.06), // Hover state
    },

    border: {
      subtle: mix(primaryBg, syntaxFg, 0.08),
      muted: borderBase,
      emphasis: mix(primaryBg, syntaxFg, isDark ? 0.25 : 0.2),
    },

    text: {
      primary: syntaxFg,
      muted: syntaxComment,
      faint: mix(primaryBg, syntaxComment, 0.5),
      accent: accentBlue,
    },

    status: {
      modified: accentOrange,
      added: accentGreen,
      deleted: accentRed,
      renamed: accentBlue,
      untracked: syntaxComment,
    },

    diff: {
      // Very subtle tints - the syntax highlighting should dominate
      addedBg: overlay(accentGreen, isDark ? 0.08 : 0.1),
      removedBg: overlay(accentRed, isDark ? 0.08 : 0.1),
      // Neutral highlight for changed lines - subtle foreground tint
      changedBg: overlay(syntaxFg, isDark ? 0.04 : 0.06),
      // Range borders need to be visible but not distracting
      rangeBorder: mix(primaryBg, syntaxFg, isDark ? 0.2 : 0.15),
      // Comment highlight - uses blue accent for "annotation" semantic
      commentHighlight: overlay(accentBlue, isDark ? 0.5 : 0.4),
    },

    search: {
      // Yellow-ish highlight for search matches - visible but not overwhelming
      matchBg: isDark ? 'rgba(250, 200, 50, 0.35)' : 'rgba(250, 200, 50, 0.5)',
      // Orange-ish highlight for current match - more prominent
      currentMatchBg: isDark ? 'rgba(255, 150, 50, 0.5)' : 'rgba(255, 150, 50, 0.6)',
    },

    annotation: {
      // Semi-transparent background for blur overlay - darker for readability
      overlayBg: isDark ? 'rgba(0, 0, 0, 0.5)' : 'rgba(255, 255, 255, 0.6)',
      // Subtle border for overlay edges
      border: isDark ? 'rgba(255, 255, 255, 0.1)' : 'rgba(0, 0, 0, 0.1)',
    },

    ui: {
      accent: accentGreen,
      accentHover: isDark ? adjust(accentGreen, -0.15) : adjust(accentGreen, 0.15),
      danger: accentRed,
      dangerBg: overlay(accentRed, isDark ? 0.1 : 0.08),
      // Selection uses foreground color for a neutral, theme-consistent highlight
      selection: overlay(syntaxFg, isDark ? 0.08 : 0.1),
    },

    scrollbar: {
      thumb: borderBase,
      thumbHover: mix(primaryBg, syntaxFg, 0.25),
      thumbTransparent: overlay(syntaxFg, isDark ? 0.15 : 0.12),
      thumbHoverTransparent: overlay(syntaxFg, isDark ? 0.25 : 0.2),
    },

    shadow: {
      // Overlay for modal backdrops - darker for light themes, lighter for dark
      overlay: isDark ? 'rgba(0, 0, 0, 0.6)' : 'rgba(0, 0, 0, 0.4)',
      // Elevated element shadows - use theme bg for colored shadow
      elevated: isDark
        ? `0 8px 24px ${overlay('#000000', 0.4)}`
        : `0 8px 24px ${overlay('#000000', 0.15)}`,
      // Selection glow - subtle glow using foreground color
      glow: `0 0 0 1px ${overlay(syntaxFg, isDark ? 0.1 : 0.08)}`,
    },
  };
}

/**
 * Generate CSS custom properties from theme.
 */
export function themeToCssVars(t: Theme): string {
  return `
    --theme-is-dark: ${t.isDark ? '1' : '0'};

    --bg-primary: ${t.bg.primary};
    --bg-chrome: ${t.bg.chrome};
    --bg-deepest: ${t.bg.deepest};
    --bg-elevated: ${t.bg.elevated};
    --bg-hover: ${t.bg.hover};

    --border-subtle: ${t.border.subtle};
    --border-muted: ${t.border.muted};
    --border-emphasis: ${t.border.emphasis};

    --text-primary: ${t.text.primary};
    --text-muted: ${t.text.muted};
    --text-faint: ${t.text.faint};
    --text-accent: ${t.text.accent};

    --status-modified: ${t.status.modified};
    --status-added: ${t.status.added};
    --status-deleted: ${t.status.deleted};
    --status-renamed: ${t.status.renamed};
    --status-untracked: ${t.status.untracked};

    --diff-added-bg: ${t.diff.addedBg};
    --diff-removed-bg: ${t.diff.removedBg};
    --diff-changed-bg: ${t.diff.changedBg};
    --diff-range-border: ${t.diff.rangeBorder};
    --diff-comment-highlight: ${t.diff.commentHighlight};

    --search-match-bg: ${t.search.matchBg};
    --search-current-match-bg: ${t.search.currentMatchBg};

    --annotation-overlay-bg: ${t.annotation.overlayBg};
    --annotation-border: ${t.annotation.border};

    --ui-accent: ${t.ui.accent};
    --ui-accent-hover: ${t.ui.accentHover};
    --ui-danger: ${t.ui.danger};
    --ui-danger-bg: ${t.ui.dangerBg};
    --ui-selection: ${t.ui.selection};

    --scrollbar-thumb: ${t.scrollbar.thumb};
    --scrollbar-thumb-hover: ${t.scrollbar.thumbHover};
    --scrollbar-thumb-transparent: ${t.scrollbar.thumbTransparent};
    --scrollbar-thumb-hover-transparent: ${t.scrollbar.thumbHoverTransparent};

    --shadow-overlay: ${t.shadow.overlay};
    --shadow-elevated: ${t.shadow.elevated};
    --shadow-glow: ${t.shadow.glow};
  `.trim();
}
