import { beforeEach, describe, expect, it, vi } from 'vitest';

// The context itself is Svelte's; here only the state object and the
// out-of-layer default are under test (see WorkspaceLayer.svelte for the
// component that provides it).
let context: unknown;
vi.mock('svelte', () => ({
  getContext: () => context,
  setContext: (_key: unknown, value: unknown) => {
    context = value;
  },
}));

import { createWorkspaceLayerState, setWorkspaceLayer, useWorkspaceLayer } from './workspaceLayer';

beforeEach(() => {
  context = undefined;
});

describe('useWorkspaceLayer', () => {
  it('is active, visible and never re-activated outside any layer', () => {
    const layer = useWorkspaceLayer();

    expect(layer.active).toBe(true);
    expect(layer.visible).toBe(true);
    expect(layer.activations).toBe(0);
  });

  it('returns the layer set by the enclosing WorkspaceLayer', () => {
    const state = createWorkspaceLayerState({
      active: () => false,
      parked: () => false,
      activations: () => 2,
    });
    setWorkspaceLayer(state);

    expect(useWorkspaceLayer()).toBe(state);
  });
});

describe('createWorkspaceLayerState', () => {
  it('reads its sources live and is visible only when active and not parked', () => {
    let active = true;
    let parked = false;
    let activations = 0;
    const layer = createWorkspaceLayerState({
      active: () => active,
      parked: () => parked,
      activations: () => activations,
    });

    expect(layer.visible).toBe(true);

    parked = true;
    expect(layer.active).toBe(true);
    expect(layer.visible).toBe(false);

    parked = false;
    active = false;
    expect(layer.active).toBe(false);
    expect(layer.visible).toBe(false);

    active = true;
    activations = 1;
    expect(layer.visible).toBe(true);
    expect(layer.activations).toBe(1);
  });
});
