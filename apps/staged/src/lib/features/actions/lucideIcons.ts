/**
 * The one module allowed to reach for Lucide's complete icon set, plus the
 * per-action-type icon every surface falls back to.
 *
 * A pinned action can carry any Lucide icon name, so the picker has to be able
 * to render all ~1,750 of them — but pulling that map into the main graph would
 * drag every icon into the app bundle. `loadIconMap()` is a single cached
 * dynamic import, which Vite code-splits into one lazily fetched ~600 kB chunk:
 * nothing loads until a card renders a custom icon or the picker opens.
 * **Never statically import that map** — everything else in the app keeps using
 * per-icon `@lucide/svelte/icons/x` imports.
 *
 * It has to be `@lucide/svelte/icons/index` rather than the `@lucide/svelte`
 * barrel: the barrel is *statically* imported elsewhere (the diff-viewer
 * package), so a dynamic import of it can't move into its own chunk and the
 * whole icon set lands in the main bundle instead — measurably, +620 kB.
 *
 * Stored icon names are kebab-case, matching the names on lucide.dev; the map
 * is keyed by PascalCase export names, which [`./iconNames`] bridges.
 */

import Play from '@lucide/svelte/icons/play';
import Hammer from '@lucide/svelte/icons/hammer';
import FlaskConical from '@lucide/svelte/icons/flask-conical';
import CheckCircle from '@lucide/svelte/icons/check-circle';
import Wrench from '@lucide/svelte/icons/wrench';
import Zap from '@lucide/svelte/icons/zap';
import Wand2 from '@lucide/svelte/icons/wand-2';
import Trash2 from '@lucide/svelte/icons/trash-2';
import { pascalToKebab } from './iconNames';

export type IconComponent = typeof Play;

/**
 * The icon an action shows when it has picked none of its own — also the
 * fallback when a stored name is no longer a Lucide icon. Shared by the card
 * header, the Actions submenu and the settings list, so an action type looks
 * the same everywhere.
 */
export function getActionTypeIcon(actionType: string): IconComponent {
  switch (actionType) {
    case 'prerun':
      return Zap;
    case 'run':
      return Play;
    case 'build':
      return Hammer;
    case 'format':
      return Wand2;
    case 'check':
      return CheckCircle;
    case 'test':
      return FlaskConical;
    case 'cleanUp':
      return Trash2;
    default:
      return Wrench;
  }
}

type LucideIconMap = Record<string, IconComponent>;

let iconMapPromise: Promise<LucideIconMap> | null = null;

/**
 * Every Lucide icon keyed by kebab-case name, fetched once and shared by every
 * caller afterwards.
 */
export function loadIconMap(): Promise<LucideIconMap> {
  iconMapPromise ??= import('@lucide/svelte/icons/index').then((module) => {
    const byKebab: LucideIconMap = {};
    for (const [pascal, component] of Object.entries(module)) {
      byKebab[pascalToKebab(pascal)] = component as IconComponent;
    }
    return byKebab;
  });
  return iconMapPromise;
}

/** One icon by kebab-case name, or null when Lucide has no such icon. */
export async function loadIconComponent(name: string): Promise<IconComponent | null> {
  return (await loadIconMap())[name] ?? null;
}
