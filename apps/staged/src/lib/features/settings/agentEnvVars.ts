/**
 * Per-agent environment variables (Settings → Doctor → Environment variables).
 *
 * Each agent keeps an ordered list of variables; a row can be switched off
 * without being deleted. The list is persisted by the preferences store and
 * read directly from `preferences.json` by the backend (see the Rust
 * `agent::env_vars` module), which adds the enabled rows to the agent's
 * process environment on launch — so the on-disk shape here must stay in sync
 * with the Rust `AgentEnvVar`. Kept separate from preferences.svelte.ts so it
 * can be unit-tested without the Svelte runtime.
 */

export interface AgentEnvVar {
  name: string;
  value: string;
  enabled: boolean;
}

/** Lists keyed by agent (provider) id, e.g. `claude`. */
export type AgentEnvVars = Record<string, AgentEnvVar[]>;

/**
 * The stored value as the preferences state: a missing, malformed or
 * non-object value reads as no variables for any agent, and an agent whose
 * list is malformed reads as having none. Missing `enabled` reads as enabled;
 * a missing `value` as empty.
 */
export function normalizeAgentEnvVars(value: unknown): AgentEnvVars {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const result: AgentEnvVars = {};
  for (const [agentId, entries] of Object.entries(value as Record<string, unknown>)) {
    const list = normalizeAgentEnvVarList(entries);
    if (list) result[agentId] = list;
  }
  return result;
}

function normalizeAgentEnvVarList(value: unknown): AgentEnvVar[] | null {
  if (!Array.isArray(value)) return null;
  const list: AgentEnvVar[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return null;
    const { name, value: entryValue, enabled } = entry as Record<string, unknown>;
    if (typeof name !== 'string') return null;
    list.push({
      name,
      value: typeof entryValue === 'string' ? entryValue : '',
      enabled: typeof enabled === 'boolean' ? enabled : true,
    });
  }
  return list;
}

/**
 * Why `name` can't be set as an environment variable, or `null` when it can.
 * Mirrors `is_valid_env_var_name` in the backend, which skips any row this
 * rejects: the row is shown with this message instead of being dropped.
 */
export function validateEnvVarName(name: string): string | null {
  if (name.length === 0) return 'Enter a name';
  if (name.includes('=')) return 'Names can’t contain =';
  if (/\s/.test(name)) return 'Names can’t contain spaces';
  if (name.includes('\0')) return 'Names can’t contain NUL';
  return null;
}
