import { getContext, setContext } from 'svelte';

/**
 * What a view mounted inside a WorkspaceLayer can ask about its layer.
 *
 * The workspace holds two stacked layers (App.svelte): the home grid and the
 * project view. Only one is active — shown by the base route — but both can
 * stay mounted, so a view can no longer treat "mounted" as "on screen".
 * Window-level handlers (⌘N, ⌘1–9), rAF loops and dialogs gate on `visible`;
 * work a remount used to redo on each visit keys on `activations`.
 */
export interface WorkspaceLayerState {
  /** The base route shows this layer. */
  readonly active: boolean;
  /** Active and the workspace isn't parked under settings or the diff view. */
  readonly visible: boolean;
  /** How many times this layer has gone inactive → active since mount. */
  readonly activations: number;
}

/**
 * Outside any layer — settings, the diff view, portals — a view is on screen
 * whenever it is mounted, so the default says so.
 */
const OUTSIDE_LAYER: WorkspaceLayerState = { active: true, visible: true, activations: 0 };

const KEY = Symbol('workspace-layer');

/** Live getters over the layer's reactive sources, for setWorkspaceLayer. */
export function createWorkspaceLayerState(sources: {
  active: () => boolean;
  parked: () => boolean;
  activations: () => number;
}): WorkspaceLayerState {
  return {
    get active() {
      return sources.active();
    },
    get visible() {
      return sources.active() && !sources.parked();
    },
    get activations() {
      return sources.activations();
    },
  };
}

export function setWorkspaceLayer(layer: WorkspaceLayerState): void {
  setContext(KEY, layer);
}

export function useWorkspaceLayer(): WorkspaceLayerState {
  return getContext<WorkspaceLayerState | undefined>(KEY) ?? OUTSIDE_LAYER;
}

/**
 * The host of the layer the base route shows: focusable (tabindex -1) and the
 * place focus goes when the control it belongs on is gone. Layers don't carry
 * aria-hidden for the parked state (the workspace above them does), so this
 * resolves under settings / diff too.
 */
export function activeLayerHost(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.workspace-layer:not([aria-hidden])');
}
