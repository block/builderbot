/**
 * Shared Constants
 *
 * Central location for magic numbers and configuration values
 * used across multiple modules.
 *
 * Rebuildable: When adding new features that need shared constants,
 * add them here rather than hardcoding in components.
 */

// =============================================================================
// Layout
// =============================================================================

/**
 * Default line height in pixels.
 * Used by scroll sync and connector drawing.
 * Note: DiffViewer measures actual line height from DOM for accuracy,
 * but this serves as a fallback and initial estimate.
 */
export const DEFAULT_LINE_HEIGHT = 20;

// =============================================================================
// Performance
// =============================================================================

/**
 * Number of alignments to process per batch during progressive loading.
 * Higher = faster initial render but more jank.
 * Lower = smoother loading but slower to complete.
 */
export const ALIGNMENT_BATCH_SIZE = 20;

/**
 * Debounce delay (ms) for scroll sync primary pane timeout.
 * After this delay without scrolling, either pane can become primary.
 */
export const SCROLL_SYNC_DEBOUNCE_MS = 150;

/**
 * Minimum pixel difference to trigger scroll sync update.
 * Prevents micro-adjustments that cause jitter.
 */
export const SCROLL_THRESHOLD_PX = 2;

// =============================================================================
// UI Scaling
// =============================================================================

/**
 * Size adjustment step in pixels.
 */
export const SIZE_STEP = 1;

/**
 * Minimum allowed size base in pixels.
 */
export const SIZE_MIN = 10;

/**
 * Maximum allowed size base in pixels.
 */
export const SIZE_MAX = 24;

/**
 * Default size base in pixels.
 */
export const SIZE_DEFAULT = 13;

// =============================================================================
// Animation
// =============================================================================

/**
 * Duration (ms) for panel flex transitions.
 * Used to schedule connector redraws after layout changes.
 */
export const PANEL_TRANSITION_MS = 250;

// =============================================================================
// Scroll Sync
// =============================================================================

/**
 * Anchor point as fraction of viewport height.
 * 0.33 = 1/3 from top, keeps context visible above current line.
 */
export const SCROLL_ANCHOR_FRACTION = 1 / 3;
