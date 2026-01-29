/**
 * Agent chat state store.
 *
 * Each tab has its own independent agent state, allowing multiple
 * concurrent chat sessions across tabs. The AgentState is passed
 * directly as a prop through the component chain.
 */

import type { AcpProviderInfo } from '../services/ai';

export type AcpProvider = 'goose' | 'claude';

/**
 * State for a single agent chat session.
 * Each tab gets its own instance.
 */
export interface AgentState {
  input: string;
  response: string;
  loading: boolean;
  error: string;
  sessionId: string | null;
  provider: AcpProvider;
}

/**
 * Global state shared across all tabs (provider discovery).
 */
export const agentGlobalState = $state({
  availableProviders: [] as AcpProviderInfo[],
  providersLoaded: false,
});

/**
 * Create a fresh agent state for a new tab.
 */
export function createAgentState(): AgentState {
  return {
    input: '',
    response: '',
    loading: false,
    error: '',
    sessionId: null,
    provider: 'goose',
  };
}
