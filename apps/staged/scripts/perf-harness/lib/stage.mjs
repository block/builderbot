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

const HARNESS_DIR = dirname(dirname(fileURLToPath(import.meta.url)));
const clone = (from, to) => cpSync(from, to, { recursive: true, mode: constants.COPYFILE_FICLONE });
const script = (path, text) => {
  writeFileSync(path, text);
  chmodSync(path, 0o755);
};

/** Returns the paths the driver needs, all inside `runDir`. */
export function stage({ runDir, built, profile }) {
  for (const dir of ['bin', 'tmp', 'logs', 'out', 'agent-state', 'acp-tools', 'origins', 'browser'])
    mkdirSync(join(runDir, dir), { recursive: true });

  // Tauri treats a binary under `target/<profile>` as a dev build and finds
  // its resources beside it, so the layout is kept.
  const appDir = join(runDir, 'target', profile);
  mkdirSync(appDir, { recursive: true });
  const binary = join(appDir, 'Staged');
  copyFileSync(built.binary, binary, constants.COPYFILE_FICLONE);
  for (const extra of ['resources', 'stagedctl']) {
    if (existsSync(join(built.binaryDir, extra)))
      clone(join(built.binaryDir, extra), join(appDir, extra));
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
  // The app runs `claude-agent-acp` from STAGED_ACP_TOOLS_DIR as a session's agent.
  const agentConfig = join(runDir, 'agent-config.json');
  script(
    join(runDir, 'acp-tools', 'claude-agent-acp'),
    `#!/bin/sh\nexec "${node}" "${join(runDir, 'harness', 'agent', 'fake-agent.cjs')}" --config "${agentConfig}" --state "${join(runDir, 'agent-state')}" "$@"\n`
  );
  return {
    binary,
    node,
    agentConfig,
    dist: join(runDir, 'dist'),
    driver: join(runDir, 'harness', 'driver', 'main.mjs'),
  };
}
