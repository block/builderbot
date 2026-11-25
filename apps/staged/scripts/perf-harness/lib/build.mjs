// Building the tree under test: the app with the `perf-harness` feature and
// the production frontend. Both end up outside the checkout, in a build
// cache per tree, so `--skip-build` can reuse them.
//
// Cargo builds into a target directory of the harness's own, never the one
// ordinary builds of the tree use: that one may be shared between worktrees
// and hold the binary a running dev app was started from, which a build
// with another feature set and dev URL would replace.

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
import { join, resolve } from 'node:path';

import { resolveExisting } from './safety.mjs';

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

/** Where the harness builds unless told otherwise: one directory for every tree, so dependencies compile once. */
export const defaultTargetDir = () =>
  join(realpathSync(tmpdir()), 'staged-perf-harness', 'cargo-target');

/**
 * The target directory to build in, or an error when it is the one ordinary
 * builds of the tree use (`treeTargetDir`, from `cargo metadata` under the
 * caller's own environment and configuration).
 */
export function chooseTargetDir({ requested, treeTargetDir }) {
  const chosen = resolveExisting(requested ? resolve(requested) : defaultTargetDir());
  if (chosen === resolveExisting(treeTargetDir)) {
    throw new Error(
      `${chosen} is the cargo target directory ordinary builds of this tree use (from CARGO_TARGET_DIR or the tree's cargo configuration). A harness build there replaces the app binary a running dev app may have been started from; name another directory with --target-dir.`
    );
  }
  return chosen;
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
export function build({ appDir, profile, port, skipBuild, targetDir: requested }) {
  const cache = buildCacheDir(appDir);
  const dist = join(cache, 'dist');
  const tauri = join(appDir, 'src-tauri');
  // A run uses the copy of the binary kept here, whatever is built later.
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
  // Asked under the caller's environment, so this is where the tree's
  // ordinary builds go (a `CARGO_TARGET_DIR`, a cargo config, or `target`).
  const metadata = JSON.parse(
    execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], {
      cwd: tauri,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
    })
  );
  const targetDir = chooseTargetDir({ requested, treeTargetDir: metadata.target_directory });
  // The variable rather than `--target-dir`, so the sidecar script's own
  // cargo calls build there too.
  const env = { ...process.env, TAURI_CONFIG: tauriConfig(port), CARGO_TARGET_DIR: targetDir };
  console.error(`[perf-harness] cargo target directory: ${targetDir}`);
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
    env,
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
  const from = join(targetDir, profile);
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
