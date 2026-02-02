/**
 * Keyboard Service
 *
 * Central registry for global keyboard shortcuts.
 * Modal-specific shortcuts (Escape to close, Enter to confirm, list navigation)
 * stay in their components - this is for app-wide shortcuts.
 *
 * Features:
 * - Single keydown listener on window
 * - Automatic input/textarea filtering
 * - Modal suppression (some shortcuts disabled when modals open)
 * - Data source for KeyboardShortcutsModal
 */

/** Modifier keys for a shortcut */
export interface Modifiers {
  ctrl?: boolean;
  meta?: boolean; // Cmd on Mac
  shift?: boolean;
  alt?: boolean;
}

/** Definition of a keyboard shortcut */
export interface Shortcut {
  /** Unique identifier */
  id: string;
  /** Key(s) that trigger this shortcut (e.g., 'j', 'ArrowDown') */
  keys: string[];
  /** Required modifiers */
  modifiers?: Modifiers;
  /** Human-readable description for the shortcuts modal */
  description: string;
  /** Category for grouping in the modal */
  category: 'navigation' | 'view' | 'comments' | 'files' | 'search';
  /** Handler function */
  handler: () => void;
  /** If true, this shortcut works even when focus is in an input/textarea */
  allowInInputs?: boolean;
}

/** Registered shortcuts */
const shortcuts: Map<string, Shortcut> = new Map();

/** Default bindings (stored when first registered) */
const defaultBindings: Map<string, { keys: string[]; modifiers?: Modifiers }> = new Map();

/** Track if listener is attached */
let listenerAttached = false;

/**
 * Check if the current platform is Mac.
 */
export function isMac(): boolean {
  return navigator.platform.toUpperCase().indexOf('MAC') >= 0;
}

/**
 * Format a shortcut's keys for display.
 */
/** A formatted key combo for display */
export interface FormattedKey {
  /** Modifier symbols/text to show (e.g., ['⌘'] or ['Ctrl', 'Shift']) */
  modifiers: string[];
  /** The main key (e.g., 'C', '↓', '+') */
  key: string;
}

/**
 * Format a shortcut's keys for display.
 * Returns structured data for flexible rendering.
 */
export function formatShortcutKeys(shortcut: Shortcut): FormattedKey[] {
  const results: FormattedKey[] = [];
  const mod = shortcut.modifiers;

  for (const key of shortcut.keys) {
    const modifiers: string[] = [];

    // Mac uses symbols in standard order: ⌃⌥⇧⌘
    // Windows/Linux spells out modifiers
    if (isMac()) {
      if (mod?.ctrl) modifiers.push('⌃');
      if (mod?.alt) modifiers.push('⌥');
      if (mod?.shift) modifiers.push('⇧');
      if (mod?.meta) modifiers.push('⌘');
    } else {
      if (mod?.ctrl) modifiers.push('Ctrl');
      if (mod?.meta) modifiers.push('Ctrl'); // meta maps to Ctrl on Windows
      if (mod?.alt) modifiers.push('Alt');
      if (mod?.shift) modifiers.push('Shift');
    }

    // Format the main key nicely
    let displayKey: string;
    if (key === 'ArrowDown') displayKey = '↓';
    else if (key === 'ArrowUp') displayKey = '↑';
    else if (key === 'ArrowLeft') displayKey = '←';
    else if (key === 'ArrowRight') displayKey = '→';
    else if (key === '=' || key === '+') displayKey = '+';
    else if (key === '-')
      displayKey = '−'; // proper minus sign
    else displayKey = key.toUpperCase();

    results.push({ modifiers, key: displayKey });
  }

  return results;
}
/**
 * Get all registered shortcuts.
 */
export function getAllShortcuts(): Shortcut[] {
  return Array.from(shortcuts.values());
}

/**
 * Check if modifiers match.
 */
function modifiersMatch(event: KeyboardEvent, mods?: Modifiers): boolean {
  const wantCtrl = mods?.ctrl ?? false;
  const wantMeta = mods?.meta ?? false;
  const wantShift = mods?.shift ?? false;
  const wantAlt = mods?.alt ?? false;

  // On Mac: meta = Cmd key
  // On Windows/Linux: meta maps to Ctrl (so ⌘C works as Ctrl+C)
  const metaKey = isMac() ? event.metaKey : event.ctrlKey;
  const ctrlKey = isMac() ? event.ctrlKey : false; // Ctrl on Mac is separate

  // Check required modifiers are pressed
  if (wantMeta && !metaKey) return false;
  if (wantCtrl && !ctrlKey) return false;
  if (wantShift && !event.shiftKey) return false;
  if (wantAlt && !event.altKey) return false;

  // Check unwanted modifiers aren't pressed
  if (!wantMeta && metaKey) return false;
  if (!wantCtrl && ctrlKey) return false;
  if (!wantAlt && event.altKey) return false;

  // For shift, only be lenient with certain keys that require shift to type
  // (like '+' which needs Shift+= on most keyboards)
  const shiftTypedKeys = ['+', '=', '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_'];
  const isShiftTypedKey = shiftTypedKeys.includes(event.key);

  // If shift is not wanted and shift is pressed, only allow if it's needed to type the key
  if (!wantShift && event.shiftKey && !isShiftTypedKey) return false;

  return true;
}

/**
 * Handle keydown events.
 */
function handleKeydown(event: KeyboardEvent): void {
  const target = event.target as HTMLElement;
  const inInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';

  // Find matching shortcut
  for (const shortcut of shortcuts.values()) {
    // Skip if in input/textarea unless explicitly allowed
    if (inInput && !shortcut.allowInInputs) {
      continue;
    }

    // Check if key matches
    const keyMatches = shortcut.keys.some(
      (k) => k.toLowerCase() === event.key.toLowerCase() || k === event.key
    );
    if (!keyMatches) continue;

    // Check modifiers
    if (!modifiersMatch(event, shortcut.modifiers)) continue;

    // Found a match!
    console.log('[keyboard] shortcut matched:', shortcut.id, 'key:', event.key);
    event.preventDefault();
    shortcut.handler();
    return;
  }
}

/**
 * Ensure the global listener is attached.
 */
function ensureListener(): void {
  if (!listenerAttached) {
    window.addEventListener('keydown', handleKeydown);
    listenerAttached = true;
  }
}

/**
 * Register a keyboard shortcut.
 * Returns an unregister function.
 */
export function registerShortcut(shortcut: Shortcut): () => void {
  ensureListener();

  // Store default binding on first registration
  if (!defaultBindings.has(shortcut.id)) {
    defaultBindings.set(shortcut.id, {
      keys: [...shortcut.keys],
      modifiers: shortcut.modifiers ? { ...shortcut.modifiers } : undefined,
    });
  }

  shortcuts.set(shortcut.id, shortcut);

  return () => {
    shortcuts.delete(shortcut.id);
  };
}

/**
 * Register multiple shortcuts at once.
 * Returns an unregister function that removes all of them.
 */
export function registerShortcuts(shortcutList: Shortcut[]): () => void {
  const unregisters = shortcutList.map((s) => registerShortcut(s));
  return () => unregisters.forEach((fn) => fn());
}

// =============================================================================
// Binding Management
// =============================================================================

/**
 * Get the default binding for a shortcut.
 */
export function getDefaultBinding(
  id: string
): { keys: string[]; modifiers?: Modifiers } | undefined {
  return defaultBindings.get(id);
}

/**
 * Check if a shortcut has been customized from its default.
 */
export function isCustomized(id: string): boolean {
  const shortcut = shortcuts.get(id);
  const defaultBinding = defaultBindings.get(id);
  if (!shortcut || !defaultBinding) return false;

  // Compare keys
  if (shortcut.keys.length !== defaultBinding.keys.length) return true;
  if (!shortcut.keys.every((k, i) => k === defaultBinding.keys[i])) return true;

  // Compare modifiers
  const sMod = shortcut.modifiers || {};
  const dMod = defaultBinding.modifiers || {};
  if (!!sMod.ctrl !== !!dMod.ctrl) return true;
  if (!!sMod.meta !== !!dMod.meta) return true;
  if (!!sMod.shift !== !!dMod.shift) return true;
  if (!!sMod.alt !== !!dMod.alt) return true;

  return false;
}

/**
 * Update a shortcut's binding at runtime.
 */
export function updateBinding(id: string, keys: string[], modifiers?: Modifiers): boolean {
  const shortcut = shortcuts.get(id);
  if (!shortcut) return false;

  shortcut.keys = keys;
  shortcut.modifiers = modifiers;
  return true;
}

/**
 * Reset a shortcut to its default binding.
 */
export function resetBinding(id: string): boolean {
  const shortcut = shortcuts.get(id);
  const defaultBinding = defaultBindings.get(id);
  if (!shortcut || !defaultBinding) return false;

  shortcut.keys = [...defaultBinding.keys];
  shortcut.modifiers = defaultBinding.modifiers ? { ...defaultBinding.modifiers } : undefined;
  return true;
}

/**
 * Check if a binding conflicts with existing shortcuts.
 * Returns the ID of the conflicting shortcut, or null if no conflict.
 */
export function hasConflict(
  keys: string[],
  modifiers?: Modifiers,
  excludeId?: string
): string | null {
  for (const [id, shortcut] of shortcuts) {
    if (id === excludeId) continue;

    // Check if any key matches
    const keyMatches = keys.some((k) =>
      shortcut.keys.some((sk) => sk.toLowerCase() === k.toLowerCase())
    );
    if (!keyMatches) continue;

    // Check if modifiers match
    const sMod = shortcut.modifiers || {};
    const tMod = modifiers || {};
    const modifiersMatch =
      !!sMod.ctrl === !!tMod.ctrl &&
      !!sMod.meta === !!tMod.meta &&
      !!sMod.shift === !!tMod.shift &&
      !!sMod.alt === !!tMod.alt;

    if (modifiersMatch) {
      return id;
    }
  }
  return null;
}

/**
 * Load and apply custom bindings from storage.
 * Call this after all shortcuts are registered.
 */
export function loadCustomBindings(
  bindings: Record<string, { keys: string[]; modifiers?: Modifiers }>
): void {
  for (const [id, binding] of Object.entries(bindings)) {
    updateBinding(id, binding.keys, binding.modifiers);
  }
}
