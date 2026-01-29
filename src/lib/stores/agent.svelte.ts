/**
 * Agent chat state store - persists across component re-renders.
 */

export const agentState = $state({
  input: '',
  response: '',
  loading: false,
  error: '',
  sessionId: null as string | null,
});

export function resetAgentState() {
  agentState.input = '';
  agentState.response = '';
  agentState.loading = false;
  agentState.error = '';
  agentState.sessionId = null;
}
