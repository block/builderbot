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
 * list isn't a list reads as having none. Within a list, only the entries
 * that aren't usable are skipped — the rest are kept, since `setAgentEnvVars`
 * writes the whole record back and anything dropped here would be erased
 * from disk the next time any agent's variables are edited. Missing
 * `enabled` reads as enabled; a missing `value` as empty.
 */
export function normalizeAgentEnvVars(value: unknown): AgentEnvVars {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const result: AgentEnvVars = {};
  for (const [agentId, entries] of Object.entries(value as Record<string, unknown>)) {
    if (!Array.isArray(entries)) continue;
    result[agentId] = entries.flatMap((entry) => normalizeAgentEnvVar(entry) ?? []);
  }
  return result;
}

/** One stored entry, or `null` when it isn't an object with a string name. */
function normalizeAgentEnvVar(entry: unknown): AgentEnvVar | null {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return null;
  const { name, value, enabled } = entry as Record<string, unknown>;
  if (typeof name !== 'string') return null;
  return {
    name,
    value: typeof value === 'string' ? value : '',
    enabled: typeof enabled === 'boolean' ? enabled : true,
  };
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

/**
 * Variable names the launcher sets itself for a local agent process, in the
 * Rust `acp_tools::apply_managed_tools_env` (session_runner shapes both env
 * snapshots through it): the managed ACP bridge directories are prepended to
 * `PATH` and listed in `GOOSE_SEARCH_PATHS`. A Doctor row with one of these
 * names is applied after that, so it replaces Staged's value for the agent
 * and everything the agent spawns. Keep in sync with that function.
 */
export const LAUNCHER_RESERVED_NAMES: ReadonlySet<string> = new Set(['PATH', 'GOOSE_SEARCH_PATHS']);

/**
 * The non-blocking warning for a row named after something the launcher
 * sets, or `null`. Unlike a row error, the backend still applies the row:
 * the user may well mean to override what Staged sets, so this only says what
 * will happen. Disabled rows and rows the backend would skip get none.
 */
export function launcherReservedNameWarning(entry: AgentEnvVar): string | null {
  if (!entry.enabled || validateEnvVar(entry) !== null) return null;
  if (!LAUNCHER_RESERVED_NAMES.has(entry.name)) return null;
  return `Replaces the ${entry.name} Staged sets for this agent`;
}

/**
 * The warning to show under each row of `entries`, by position; `null` for a
 * row with none. A row that has an error (see `agentEnvVarRowErrors`) gets no
 * warning, since the error already says the row won't be applied.
 */
export function agentEnvVarRowWarnings(entries: readonly AgentEnvVar[]): (string | null)[] {
  return entries.map((entry) => launcherReservedNameWarning(entry));
}
