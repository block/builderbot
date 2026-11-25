// Copies everything a run executes or reads into the throwaway directory, so
// that neither the app nor the driver has a file open in the checkout (which
// may itself live under `~/.staged/workspaces`) while it runs.

import {
  chmodSync,
  constants,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { agentCommands, agentStub, FAKE_AGENT_COMMAND } from './agents.mjs';
import { loginFiles } from './safety.mjs';

const HARNESS_DIR = dirname(dirname(fileURLToPath(import.meta.url)));
const clone = (from, to) => cpSync(from, to, { recursive: true, mode: constants.COPYFILE_FICLONE });
const script = (path, text) => {
  writeFileSync(path, text);
  chmodSync(path, 0o755);
};

/**
 * Returns the paths the driver needs, all inside `runDir`, and the agents
 * it stubbed. `appDir` is the tree the app was built from, whose source
 * says which agents the app knows.
 */
export function stage({ runDir, home, appDir, built, profile }) {
  // Before anything is copied: a tree whose agents cannot be read stops here.
  const agents = agentCommands(appDir);
  for (const dir of ['bin', 'tmp', 'logs', 'out', 'agent-state', 'acp-tools', 'origins', 'browser'])
    mkdirSync(join(runDir, dir), { recursive: true });

  // Tauri treats a binary under `target/<profile>` as a dev build and finds
  // its resources beside it, so the layout is kept.
  const binaryDir = join(runDir, 'target', profile);
  mkdirSync(binaryDir, { recursive: true });
  const binary = join(binaryDir, 'Staged');
  copyFileSync(built.binary, binary, constants.COPYFILE_FICLONE);
  for (const extra of ['resources', 'stagedctl']) {
    if (existsSync(join(built.binaryDir, extra)))
      clone(join(built.binaryDir, extra), join(binaryDir, extra));
  }
  clone(built.dist, join(runDir, 'dist'));
  for (const part of ['driver', 'lib', 'agent'])
    clone(join(HARNESS_DIR, part), join(runDir, 'harness', part));

  const node = join(runDir, 'bin', 'node');
  copyFileSync(process.execPath, node, constants.COPYFILE_FICLONE);
  chmodSync(node, 0o755);
  // Stands in for the user's login shell: the same commands, none of the
  // login files, so nothing of the user's reaches the PATH the app captures.
  script(join(runDir, 'bin', 'perf-shell'), '#!/bin/sh\nexec /bin/sh -s\n');
  // The login shell the app asks where a program is reads these, so it
  // answers from the run's PATH and not from the machine's.
  for (const [name, text] of Object.entries(loginFiles(runDir)))
    writeFileSync(join(home, name), text);
  // The app runs `claude-agent-acp` from STAGED_ACP_TOOLS_DIR as a session's agent.
  const agentConfig = join(runDir, 'agent-config.json');
  script(
    join(runDir, 'acp-tools', FAKE_AGENT_COMMAND),
    `#!/bin/sh\nexec "${node}" "${join(runDir, 'harness', 'agent', 'fake-agent.cjs')}" --config "${agentConfig}" --state "${join(runDir, 'agent-state')}" "$@"\n`
  );
  // Every other agent the app knows gets a stub there. The app looks in
  // that directory before anywhere else, so it never goes on to find the
  // user's own copy (creating a project starts the first agent it finds).
  const agentStubLog = join(runDir, 'logs', 'agent-stubs.log');
  const agentStubs = agents.filter((name) => name !== FAKE_AGENT_COMMAND);
  for (const name of agentStubs)
    script(join(runDir, 'acp-tools', name), agentStub({ name, log: agentStubLog }));
  return {
    binary,
    node,
    agentConfig,
    agentStubs,
    agentStubLog,
    dist: join(runDir, 'dist'),
    driver: join(runDir, 'harness', 'driver', 'main.mjs'),
  };
}
