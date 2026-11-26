// Which agent programs the app can go looking for, read from the source of
// the tree a run builds, and what stands in for each of them.
//
// The app finds an agent by name: first in its tools directory (the run's
// `acp-tools`, through STAGED_ACP_TOOLS_DIR), then through a real login
// shell and a list of install directories (`crates/doctor/src/resolve.rs`).
// The second half is outside the run's control, since it finds whatever
// the user has installed. So every name the tree knows gets a file in the
// tools directory, where the search ends: the fake agent for the one a
// session runs, and for the rest a stub that records the start and exits.
// The names are read from the tree and not listed here, so an agent added
// to the app is stubbed without the harness being told.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/** The command a session's agent runs as; the fake agent takes its place. */
export const FAKE_AGENT_COMMAND = 'claude-agent-acp';

/**
 * Where the tree names its agents, relative to the repository root: the
 * list a session or action detection picks from, and the list Doctor
 * probes (which adds the agents' own CLIs). `fields` are the struct fields
 * that hold a command name.
 */
export const AGENT_SOURCES = [
  { file: 'crates/acp-client/src/types.rs', list: 'KNOWN_AGENTS', fields: ['command'] },
  {
    file: 'crates/doctor/src/agents.rs',
    list: 'AI_AGENT_CHECKS',
    fields: ['commands', 'main_command'],
  },
];

/** The text of `const <list>: &[…] = &[ … ];`, or `null` when the source has no such list. */
export function listBody(source, list) {
  const start = source.search(new RegExp(`\\bconst ${list}\\s*:[^=]*=\\s*&\\[`));
  if (start < 0) return null;
  const end = source.indexOf('\n];', start);
  return end < 0 ? null : source.slice(start, end);
}

/**
 * The command names in a list's text: the string literals of each of
 * `fields`, whether the field holds one (`"x"`), an optional one
 * (`Some("x")`) or several (`&["x", "y"]`).
 */
export function commandsIn(body, fields) {
  const out = [];
  // A field is matched from the start of its name, so `command` does not
  // match inside `install_command` or `auth_command`, which hold shell
  // commands and not program names.
  const field = new RegExp(
    `(?<![A-Za-z0-9_])(?:${fields.join('|')})\\s*:\\s*(?:Some\\(\\s*)?(?:&\\[\\s*)?((?:"[^"]*"\\s*,?\\s*)+)`,
    'g'
  );
  for (const [, literals] of body.matchAll(field)) {
    for (const [, name] of literals.matchAll(/"([^"]*)"/g)) out.push(name);
  }
  return out;
}

/**
 * Every agent command the tree at `appDir` (its `apps/staged`) knows,
 * sorted. Throws when a list cannot be read or names nothing: a run that
 * cannot tell which agents the app knows must not start, since it would
 * leave them to be found on the machine.
 */
export function agentCommands(appDir) {
  const root = join(appDir, '..', '..');
  const names = new Set();
  for (const { file, list, fields } of AGENT_SOURCES) {
    let source;
    try {
      source = readFileSync(join(root, file), 'utf8');
    } catch (error) {
      throw new Error(`cannot read the agents the app knows from ${file}: ${error.message}`);
    }
    const body = listBody(source, list);
    const found = body ? commandsIn(body, fields) : [];
    if (found.length === 0)
      throw new Error(`cannot read the agents the app knows: no ${list} entries in ${file}`);
    for (const name of found) {
      // A name becomes a file in the tools directory.
      if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name))
        throw new Error(`${file} names an agent command that is not a plain file name: ${name}`);
      names.add(name);
    }
  }
  return [...names].sort();
}

/**
 * The stub for an agent the run does not use: appends its name and
 * arguments to `log` and exits, so whatever asked for the agent is told at
 * once that it is not there. Shell builtins only.
 */
export const agentStub = ({ name, log }) =>
  `#!/bin/sh
# Staged load-test harness: stands in for \`${name}\`, so the app finds this
# file and never the user's own ${name}.
printf '%s\\n' "${name} $*" >> '${log}'
echo "perf-harness: ${name} is a stub; a load run starts no real agent" >&2
exit 1
`;

/** How many times each stub was started, from the stubs' log (`{ goose: 2 }`). */
export function stubStarts(logText) {
  const out = {};
  for (const line of logText.split('\n')) {
    const name = line.split(' ')[0];
    if (name) out[name] = (out[name] ?? 0) + 1;
  }
  return out;
}
