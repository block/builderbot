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

/**
 * Why `value` can't be passed to a process, or `null` when it can. The
 * backend skips a row whose value holds a NUL byte (the OS can't represent
 * it); a text input can't produce one, but pasted or imported settings can.
 */
export function validateEnvVarValue(value: string): string | null {
  if (value.includes('\0')) return 'Values can’t contain NUL';
  return null;
}

/**
 * Why the backend would skip `entry` on launch, or `null` when it would set
 * it. The one predicate behind the inline row errors, so every condition in
 * the backend's `enabled_env_pairs` filter has a message here.
 */
export function validateEnvVar(entry: Pick<AgentEnvVar, 'name' | 'value'>): string | null {
  return validateEnvVarName(entry.name) ?? validateEnvVarValue(entry.value);
}

export const DUPLICATE_NAME_ERROR = 'Duplicate name: the last row wins';

/**
 * Names set by more than one enabled row in `entries`. Only rows the backend
 * would apply count — enabled, with a valid name and value — since a disabled
 * or skipped row never shadows another. Names are compared exactly, as the
 * backend does; environment variable names are case-sensitive.
 */
export function duplicateEnabledNames(entries: readonly AgentEnvVar[]): Set<string> {
  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const entry of entries) {
    if (!entry.enabled || validateEnvVar(entry) !== null) continue;
    if (seen.has(entry.name)) duplicates.add(entry.name);
    seen.add(entry.name);
  }
  return duplicates;
}

/**
 * The error to show under each row of `entries`, by position; `null` for a
 * row with nothing wrong. A row just added is blank by design, so an empty
 * name is only reported once something else is wrong with it or the row
 * would otherwise look complete (a value with no name). Every enabled row
 * that shares its name with another is marked, not only the later one, since
 * the user can't tell from a single row which one the backend keeps.
 */
export function agentEnvVarRowErrors(entries: readonly AgentEnvVar[]): (string | null)[] {
  const duplicates = duplicateEnabledNames(entries);
  return entries.map((entry) => {
    if (entry.name.length === 0 && entry.value.length === 0) return null;
    const error = validateEnvVar(entry);
    if (error !== null) return error;
    if (entry.enabled && duplicates.has(entry.name)) return DUPLICATE_NAME_ERROR;
    return null;
  });
}
