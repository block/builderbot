/**
 * Agent discovery cache and known-agent metadata.
 *
 * Discovered providers are loaded once at app startup and cached here.
 * Components read from this store instead of calling discoverAcpProviders()
 * on every mount. The refresh button in various UI surfaces updates this
 * cache so all components benefit immediately.
 *
 * `KNOWN_AGENTS` is the single source of truth for agent display metadata
 * on the frontend (labels, descriptions, install URLs). The backend owns
 * the registry of supported agent IDs and CLI commands.
 */

import { discoverAcpProviders, type AcpProviderInfo } from '../../api/commands';

// =============================================================================
// Known agent metadata (display-only — the backend owns the real registry)
// =============================================================================

export interface KnownAgent {
  id: string;
  label: string;
  description: string;
  installUrl: string | null;
}

/** All agents the app knows about, in display order. */
export const KNOWN_AGENTS: KnownAgent[] = [
  {
    id: 'goose',
    label: 'Goose',
    description: 'Open-source AI agent by Block',
    installUrl: 'https://github.com/block/goose',
  },
  {
    id: 'claude',
    label: 'Claude Code',
    description: 'AI coding assistant by Anthropic',
    installUrl:
      'https://github.com/zed-industries/claude-agent-acp?tab=readme-ov-file#installation',
  },
  {
    id: 'codex',
    label: 'Codex',
    description: 'AI coding agent by OpenAI',
    installUrl: 'https://github.com/zed-industries/codex-acp#installation',
  },
  {
    id: 'pi',
    label: 'Pi',
    description: 'AI coding agent by Mario Zechner',
    installUrl: null,
  },
  {
    id: 'amp',
    label: 'Amp',
    description: 'AI coding agent by Sourcegraph',
    installUrl: 'https://www.npmjs.com/package/amp-acp',
  },
];

/** Agents always available on remote Blox workstations, regardless of local installs. */
export const REMOTE_AGENTS: KnownAgent[] = KNOWN_AGENTS.filter((a) =>
  ['goose', 'claude'].includes(a.id)
);

// =============================================================================
// Reactive discovery cache
// =============================================================================

/** Shared reactive state for discovered providers. */
export const agentState = $state({
  providers: [] as AcpProviderInfo[],
  loaded: false,
});

/**
 * Discover providers and update the cache.
 *
 * Safe to call multiple times — each call replaces the cached list.
 * Called once at startup and again when the user clicks "Refresh".
 */
export async function refreshProviders(
  options: { force?: boolean } = {}
): Promise<AcpProviderInfo[]> {
  try {
    const { data: providers, revalidating } = await discoverAcpProviders(options);
    agentState.providers = providers;
    agentState.loaded = true;
    if (revalidating) {
      revalidating
        .then((fresh) => {
          agentState.providers = fresh;
        })
        .catch((e) => console.error('Failed to revalidate ACP providers:', e));
    }
    return providers;
  } catch (e) {
    console.error('Failed to discover ACP providers:', e);
    agentState.providers = [];
    agentState.loaded = true;
    return [];
  }
}
