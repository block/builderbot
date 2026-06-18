import { deleteStoreValue, getStoreValue, setStoreValue } from '../../shared/persistentStore';

const KEYBOARD_BINDINGS_STORE_KEY = 'keyboard-bindings';

export interface ShortcutModifiers {
  ctrl?: boolean;
  meta?: boolean;
  shift?: boolean;
  alt?: boolean;
}

export interface ShortcutBinding {
  keys: string[];
  modifiers?: ShortcutModifiers;
}

export type ShortcutCategory = 'app' | 'search' | 'view';

export interface Shortcut {
  id: string;
  description: string;
  category: ShortcutCategory;
  handler: () => void | boolean;
  keys: string[];
  modifiers?: ShortcutModifiers;
  allowInInputs?: boolean;
}

export interface FormattedKey {
  modifiers: string[];
  key: string;
}

type BindingMap = Record<string, ShortcutBinding>;

const shortcuts: Map<string, Shortcut> = new Map();
const defaultBindings: Map<string, ShortcutBinding> = new Map();

let savedBindings: BindingMap = {};
let bindingsLoaded = false;
let listenerAttached = false;
let suppressionDepth = 0;
const SHIFT_TYPED_KEYS = new Set(['+', '_', '!', '@', '#', '$', '%', '^', '&', '*', '(', ')']);

function cloneModifiers(modifiers?: ShortcutModifiers): ShortcutModifiers | undefined {
  if (!modifiers) return undefined;
  return {
    ctrl: !!modifiers.ctrl,
    meta: !!modifiers.meta,
    shift: !!modifiers.shift,
    alt: !!modifiers.alt,
  };
}

function normalizeKey(key: string): string {
  if (key.length === 1) return key.toLowerCase();
  if (key === 'Spacebar') return ' ';
  if (key === 'Space') return ' ';
  return key;
}

function normalizeBinding(binding: ShortcutBinding): ShortcutBinding {
  const uniqueKeys = [...new Set(binding.keys.map((key) => normalizeKey(key)).filter(Boolean))];
  return {
    keys: uniqueKeys,
    modifiers: cloneModifiers(binding.modifiers),
  };
}

function cloneBinding(binding: ShortcutBinding): ShortcutBinding {
  return {
    keys: [...binding.keys],
    modifiers: cloneModifiers(binding.modifiers),
  };
}

function modifiersEqual(a?: ShortcutModifiers, b?: ShortcutModifiers): boolean {
  return (
    !!a?.ctrl === !!b?.ctrl &&
    !!a?.meta === !!b?.meta &&
    !!a?.shift === !!b?.shift &&
    !!a?.alt === !!b?.alt
  );
}

function nonShiftModifiersEqual(a?: ShortcutModifiers, b?: ShortcutModifiers): boolean {
  return !!a?.ctrl === !!b?.ctrl && !!a?.meta === !!b?.meta && !!a?.alt === !!b?.alt;
}

function modifiersCanOverlapForKey(
  key: string,
  first?: ShortcutModifiers,
  second?: ShortcutModifiers
): boolean {
  if (!nonShiftModifiersEqual(first, second)) return false;

  const firstShift = !!first?.shift;
  const secondShift = !!second?.shift;
  if (firstShift === secondShift) return true;

  return SHIFT_TYPED_KEYS.has(normalizeKey(key));
}

function bindingsEqual(a: ShortcutBinding, b: ShortcutBinding): boolean {
  const aNorm = normalizeBinding(a);
  const bNorm = normalizeBinding(b);
  if (aNorm.keys.length !== bNorm.keys.length) return false;
  if (!aNorm.keys.every((key, index) => key === bNorm.keys[index])) return false;
  return modifiersEqual(aNorm.modifiers, bNorm.modifiers);
}

function parseSavedBindings(raw: unknown): BindingMap {
  const parsed: BindingMap = {};
  if (!raw || typeof raw !== 'object') return parsed;

  for (const [id, binding] of Object.entries(raw as Record<string, unknown>)) {
    if (!binding || typeof binding !== 'object') continue;
    const keysRaw = (binding as { keys?: unknown }).keys;
    if (!Array.isArray(keysRaw)) continue;
    const keys = keysRaw.filter((key): key is string => typeof key === 'string');
    if (keys.length === 0) continue;
    const modifiersRaw = (binding as { modifiers?: unknown }).modifiers;
    const modifiers =
      modifiersRaw && typeof modifiersRaw === 'object'
        ? {
            ctrl: !!(modifiersRaw as ShortcutModifiers).ctrl,
            meta: !!(modifiersRaw as ShortcutModifiers).meta,
            shift: !!(modifiersRaw as ShortcutModifiers).shift,
            alt: !!(modifiersRaw as ShortcutModifiers).alt,
          }
        : undefined;
    parsed[id] = normalizeBinding({ keys, modifiers });
  }

  return parsed;
}

function isInputTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tagName = target.tagName;
  return tagName === 'INPUT' || tagName === 'TEXTAREA' || tagName === 'SELECT';
}

function modifiersMatch(event: KeyboardEvent, mods?: ShortcutModifiers): boolean {
  const wantCtrl = !!mods?.ctrl;
  const wantMeta = !!mods?.meta;
  const wantShift = !!mods?.shift;
  const wantAlt = !!mods?.alt;

  // meta means Cmd on macOS and Ctrl elsewhere.
  const metaKey = isMac() ? event.metaKey : event.ctrlKey;
  // ctrl is only used on macOS in this mapping.
  const ctrlKey = isMac() ? event.ctrlKey : false;

  if (wantMeta !== metaKey) return false;
  if (wantCtrl !== ctrlKey) return false;
  if (wantAlt !== event.altKey) return false;

  if (wantShift && !event.shiftKey) return false;
  if (!wantShift && event.shiftKey) {
    // Allow shift when it is only being used to type a symbol key (for example '+').
    if (!SHIFT_TYPED_KEYS.has(event.key)) return false;
  }

  return true;
}

function keyMatches(eventKey: string, candidate: string): boolean {
  return normalizeKey(eventKey) === normalizeKey(candidate);
}

function handleKeydown(event: KeyboardEvent): void {
  if (suppressionDepth > 0 || event.defaultPrevented) return;
  const inInput = isInputTarget(event.target);

  for (const shortcut of shortcuts.values()) {
    if (inInput && !shortcut.allowInInputs) continue;

    const matchesKey = shortcut.keys.some((key) => keyMatches(event.key, key));
    if (!matchesKey) continue;
    if (!modifiersMatch(event, shortcut.modifiers)) continue;

    const handled = shortcut.handler();
    if (handled === false) return;

    event.preventDefault();
    return;
  }
}

function ensureListener(): void {
  if (listenerAttached) return;
  window.addEventListener('keydown', handleKeydown);
  listenerAttached = true;
}

function maybeDetachListener(): void {
  if (!listenerAttached || shortcuts.size > 0) return;
  window.removeEventListener('keydown', handleKeydown);
  listenerAttached = false;
}

function applySavedBindingIfPresent(shortcut: Shortcut): void {
  if (!bindingsLoaded) return;
  const saved = savedBindings[shortcut.id];
  if (!saved) return;
  shortcut.keys = [...saved.keys];
  shortcut.modifiers = cloneModifiers(saved.modifiers);
}

async function persistSavedBindings(): Promise<void> {
  if (Object.keys(savedBindings).length === 0) {
    await deleteStoreValue(KEYBOARD_BINDINGS_STORE_KEY);
    return;
  }
  await setStoreValue(KEYBOARD_BINDINGS_STORE_KEY, savedBindings);
}

function getDefaultBinding(id: string): ShortcutBinding | null {
  const binding = defaultBindings.get(id);
  return binding ? cloneBinding(binding) : null;
}

export function isMac(): boolean {
  return typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC');
}

export async function initializeShortcutBindings(): Promise<void> {
  if (bindingsLoaded) return;
  const raw = await getStoreValue<unknown>(KEYBOARD_BINDINGS_STORE_KEY);
  savedBindings = parseSavedBindings(raw);
  bindingsLoaded = true;

  for (const shortcut of shortcuts.values()) {
    applySavedBindingIfPresent(shortcut);
  }
}

export function registerShortcut(shortcut: Shortcut): () => void {
  ensureListener();

  if (!defaultBindings.has(shortcut.id)) {
    defaultBindings.set(
      shortcut.id,
      normalizeBinding({ keys: shortcut.keys, modifiers: shortcut.modifiers })
    );
  }

  const registered: Shortcut = {
    ...shortcut,
    keys: [...shortcut.keys],
    modifiers: cloneModifiers(shortcut.modifiers),
  };
  applySavedBindingIfPresent(registered);
  shortcuts.set(registered.id, registered);

  return () => {
    shortcuts.delete(registered.id);
    maybeDetachListener();
  };
}

export function registerShortcuts(shortcutList: Shortcut[]): () => void {
  const unregisters = shortcutList.map((shortcut) => registerShortcut(shortcut));
  return () => {
    for (const unregister of unregisters) {
      unregister();
    }
  };
}

export function suspendShortcutHandling(): () => void {
  suppressionDepth += 1;
  return () => {
    suppressionDepth = Math.max(0, suppressionDepth - 1);
  };
}

export function getAllShortcuts(): Shortcut[] {
  return [...shortcuts.values()]
    .map((shortcut) => ({
      ...shortcut,
      keys: [...shortcut.keys],
      modifiers: cloneModifiers(shortcut.modifiers),
    }))
    .sort((a, b) => {
      if (a.category === b.category) {
        return a.description.localeCompare(b.description);
      }
      return a.category.localeCompare(b.category);
    });
}

export function formatShortcutKeys(
  binding: Pick<ShortcutBinding, 'keys' | 'modifiers'>
): FormattedKey[] {
  const results: FormattedKey[] = [];

  for (const rawKey of binding.keys) {
    const modifiers: string[] = [];
    if (isMac()) {
      if (binding.modifiers?.ctrl) modifiers.push('⌃');
      if (binding.modifiers?.alt) modifiers.push('⌥');
      if (binding.modifiers?.shift) modifiers.push('⇧');
      if (binding.modifiers?.meta) modifiers.push('⌘');
    } else {
      const labels: string[] = [];
      if (binding.modifiers?.ctrl || binding.modifiers?.meta) labels.push('Ctrl');
      if (binding.modifiers?.alt) labels.push('Alt');
      if (binding.modifiers?.shift) labels.push('Shift');
      modifiers.push(...labels);
    }

    const key = normalizeKey(rawKey);
    let displayKey: string;
    if (key === 'ArrowDown') displayKey = '↓';
    else if (key === 'ArrowUp') displayKey = '↑';
    else if (key === 'ArrowLeft') displayKey = '←';
    else if (key === 'ArrowRight') displayKey = '→';
    else if (key === 'Escape') displayKey = 'Esc';
    else if (key === ' ') displayKey = 'Space';
    else if (key === '-') displayKey = '−';
    else displayKey = key.length === 1 ? key.toUpperCase() : key;

    results.push({ modifiers, key: displayKey });
  }

  return results;
}

export function hasShortcutConflict(
  keys: string[],
  modifiers?: ShortcutModifiers,
  excludeId?: string
): string | null {
  const target = normalizeBinding({ keys, modifiers });

  for (const [id, shortcut] of shortcuts.entries()) {
    if (id === excludeId) continue;

    const keyOverlap = target.keys.some((key) =>
      shortcut.keys.some(
        (shortcutKey) =>
          normalizeKey(shortcutKey) === key &&
          modifiersCanOverlapForKey(key, target.modifiers, shortcut.modifiers)
      )
    );
    if (keyOverlap) return id;
  }

  return null;
}

export function isShortcutCustomized(id: string): boolean {
  const shortcut = shortcuts.get(id);
  if (!shortcut) return false;
  const defaultBinding = getDefaultBinding(id);
  if (!defaultBinding) return false;
  return !bindingsEqual(
    { keys: shortcut.keys, modifiers: shortcut.modifiers },
    { keys: defaultBinding.keys, modifiers: defaultBinding.modifiers }
  );
}

export async function updateShortcutBinding(
  id: string,
  keys: string[],
  modifiers?: ShortcutModifiers
): Promise<boolean> {
  const shortcut = shortcuts.get(id);
  if (!shortcut) return false;
  if (keys.length === 0) return false;

  const nextBinding = normalizeBinding({ keys, modifiers });
  shortcut.keys = [...nextBinding.keys];
  shortcut.modifiers = cloneModifiers(nextBinding.modifiers);

  const defaultBinding = getDefaultBinding(id);
  if (!defaultBinding) return false;

  if (bindingsEqual(nextBinding, defaultBinding)) {
    delete savedBindings[id];
  } else {
    savedBindings[id] = cloneBinding(nextBinding);
  }

  await persistSavedBindings();
  return true;
}

export async function resetShortcutBinding(id: string): Promise<boolean> {
  const shortcut = shortcuts.get(id);
  const defaultBinding = getDefaultBinding(id);
  if (!shortcut || !defaultBinding) return false;

  shortcut.keys = [...defaultBinding.keys];
  shortcut.modifiers = cloneModifiers(defaultBinding.modifiers);
  delete savedBindings[id];
  await persistSavedBindings();
  return true;
}

export async function resetAllShortcutBindings(): Promise<void> {
  for (const shortcut of shortcuts.values()) {
    const defaults = getDefaultBinding(shortcut.id);
    if (!defaults) continue;
    shortcut.keys = [...defaults.keys];
    shortcut.modifiers = cloneModifiers(defaults.modifiers);
  }

  savedBindings = {};
  await persistSavedBindings();
}

export function triggerShortcut(id: string): boolean {
  const shortcut = shortcuts.get(id);
  if (!shortcut) return false;
  return shortcut.handler() !== false;
}
