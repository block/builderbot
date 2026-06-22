import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AcpProviderInfo } from '../../api/commands';

describe('refreshProviders', () => {
  let discoverAcpProviders: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    vi.stubGlobal('$state', (initial: unknown) => initial);
    discoverAcpProviders = vi.fn();
    vi.doMock('../../api/commands', () => ({
      discoverAcpProviders,
    }));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.doUnmock('../../api/commands');
  });

  it('updates provider state from forced SWR revalidation', async () => {
    const cachedProviders: AcpProviderInfo[] = [{ id: 'goose', label: 'Goose' }];
    const freshProviders: AcpProviderInfo[] = [
      { id: 'goose', label: 'Goose' },
      { id: 'codex', label: 'Codex' },
    ];
    let resolveFresh!: (providers: AcpProviderInfo[]) => void;
    const revalidating = new Promise<AcpProviderInfo[]>((resolve) => {
      resolveFresh = resolve;
    });
    discoverAcpProviders.mockResolvedValue({ data: cachedProviders, revalidating });

    const { agentState, refreshProviders } = await import('./agent.svelte');

    await expect(refreshProviders({ force: true })).resolves.toBe(cachedProviders);

    expect(discoverAcpProviders).toHaveBeenCalledWith({ force: true });
    expect(agentState.providers).toBe(cachedProviders);
    expect(agentState.loaded).toBe(true);

    resolveFresh(freshProviders);
    await revalidating;
    await Promise.resolve();

    expect(agentState.providers).toBe(freshProviders);
  });
});
