// Building the tree under test: the app with the `perf-harness` feature and
// the production frontend. Both end up outside the checkout, in a build
// cache per tree, so `--skip-build` can reuse them.

import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  constants,
  cpSync,
  existsSync,
  mkdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/** The identifier keeps the webview's own data apart from a real install's, on top of the throwaway HOME. */
const tauriConfig = (port) =>
  JSON.stringify({
    identifier: 'xyz.block.staged.perfharness',
    build: { devUrl: `http://127.0.0.1:${port}/` },
  });

export function buildCacheDir(appDir) {
  const key = createHash('sha256').update(realpathSync(appDir)).digest('hex').slice(0, 12);
  return join(realpathSync(tmpdir()), 'staged-perf-harness', 'build', key);
}

function run(what, file, args, options) {
  console.error(`[perf-harness] ${what}`);
  const result = spawnSync(file, args, { stdio: ['ignore', 2, 2], ...options });
  if (result.status !== 0)
    throw new Error(`${what} failed (${result.error?.message ?? `exit ${result.status}`})`);
}

/**
 * Build (or, with `skipBuild`, find) the app binary and the frontend.
 * Returns `{ binary, binaryDir, dist }`.
 */
export function build({ appDir, profile, port, skipBuild }) {
  const cache = buildCacheDir(appDir);
  const dist = join(cache, 'dist');
  const tauri = join(appDir, 'src-tauri');
  const env = { ...process.env, TAURI_CONFIG: tauriConfig(port) };
  const metadata = JSON.parse(
    execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], {
      cwd: tauri,
      env,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
    })
  );
  // The build goes through the tree's own target directory, where the next
  // ordinary build replaces the binary; a run uses the copy kept here.
  const binaryDir = join(cache, 'target', profile);
  const binary = join(binaryDir, 'Staged');
  const built = join(cache, `built-${profile}-${port}`);
  if (skipBuild) {
    if (!existsSync(built) || !existsSync(binary) || !existsSync(join(dist, 'index.html'))) {
      throw new Error(
        `--skip-build: nothing built for this tree, profile and port yet (looked in ${cache})`
      );
    }
    return { binary, binaryDir, dist };
  }
  mkdirSync(cache, { recursive: true });
  rmSync(built, { force: true });
  const staged = metadata.packages.find((p) => p.name === 'Staged');
  if (!staged || !('perf-harness' in staged.features)) {
    throw new Error(
      `${tauri} has no \`perf-harness\` cargo feature: this tree predates the harness hooks. Apply the commit that added src-tauri/src/perf_harness.rs to it (the touch points outside that module are listed at its top), then run again.`
    );
  }
  run('staging the stagedctl sidecar', 'bash', ['scripts/prepare-stagedctl-sidecar.sh'], {
    cwd: appDir,
  });
  run(
    `cargo build --features perf-harness (${profile})`,
    'cargo',
    ['build', '--features', 'perf-harness', ...(profile === 'release' ? ['--release'] : [])],
    { cwd: tauri, env }
  );
  run(
    'vite build',
    process.execPath,
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--outDir',
      dist,
      '--emptyOutDir',
      '--logLevel',
      'warn',
    ],
    { cwd: appDir }
  );
  const from = join(metadata.target_directory, profile);
  rmSync(binaryDir, { recursive: true, force: true });
  mkdirSync(binaryDir, { recursive: true });
  for (const part of ['Staged', 'resources', 'stagedctl']) {
    if (existsSync(join(from, part)))
      cpSync(join(from, part), join(binaryDir, part), {
        recursive: true,
        mode: constants.COPYFILE_FICLONE,
      });
  }
  writeFileSync(built, new Date().toISOString());
  return { binary, binaryDir, dist };
}

/** What was measured: the tree's commit and whether it had uncommitted changes. */
export function describeTree(appDir) {
  const git = (...args) => {
    try {
      return execFileSync('git', args, {
        cwd: appDir,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim();
    } catch {
      return null;
    }
  };
  return {
    appDir: realpathSync(appDir),
    head: git('rev-parse', 'HEAD'),
    branch: git('rev-parse', '--abbrev-ref', 'HEAD'),
    dirty: (git('status', '--porcelain') ?? '') !== '',
  };
}
