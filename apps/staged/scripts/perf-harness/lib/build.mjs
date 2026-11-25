// Building the tree under test: the app with the `perf-harness` feature and
// the production frontend. Both end up outside the checkout, in a build
// cache per tree, so `--skip-build` can reuse them.
//
// Cargo builds into a target directory of the harness's own, never the one
// ordinary builds of the tree use: that one may be shared between worktrees
// and hold the binary a running dev app was started from, which a build
// with another feature set and dev URL would replace.
//
// That directory is by default one for every tree, so a harness build of
// another tree can be writing it at the same time. A lock in it is held
// from before `cargo build` until the binary has been copied out, so what a
// run copies is what it built.
//
// The binary has its port compiled in (the dev URL), and the cache keeps
// one binary per profile. A marker per port says which port that binary was
// built for, and every marker of the profile goes when the binary is
// replaced: `--skip-build` then finds only the port of the binary that is
// really there.

import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  constants,
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { withLock } from './lock.mjs';
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

/** Where a tree's cache keeps the binary of a profile, and the marker that says which port it was built for. */
export const cachedBinaryDir = (cache, profile) => join(cache, 'target', profile);
export const builtMarker = (cache, profile, port) => join(cache, `built-${profile}-${port}`);

/**
 * Put the binary cargo just built in `from` (and what goes beside it) into
 * the cache, in place of the one there. Every marker of the profile is
 * removed first, whatever port it names: each vouched for the binary that
 * is about to go.
 */
export function replaceCachedBinary({ from, cache, profile }) {
  const binaryDir = cachedBinaryDir(cache, profile);
  mkdirSync(cache, { recursive: true });
  for (const name of readdirSync(cache)) {
    if (name.startsWith(`built-${profile}-`)) rmSync(join(cache, name), { force: true });
  }
  rmSync(binaryDir, { recursive: true, force: true });
  mkdirSync(binaryDir, { recursive: true });
  for (const part of ['Staged', 'resources', 'stagedctl']) {
    if (existsSync(join(from, part)))
      cpSync(join(from, part), join(binaryDir, part), {
        recursive: true,
        mode: constants.COPYFILE_FICLONE,
      });
  }
  return binaryDir;
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
  const binaryDir = cachedBinaryDir(cache, profile);
  const binary = join(binaryDir, 'Staged');
  const built = builtMarker(cache, profile, port);
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
  const staged = metadata.packages.find((p) => p.name === 'Staged');
  if (!staged || !('perf-harness' in staged.features)) {
    throw new Error(
      `${tauri} has no \`perf-harness\` cargo feature: this tree predates the harness hooks. Apply the commit that added src-tauri/src/perf_harness.rs to it (the touch points outside that module are listed at its top), then run again.`
    );
  }
  // Held until the binary is out of the shared directory. A build that
  // fails leaves the cache, and so the markers, as they were.
  mkdirSync(targetDir, { recursive: true });
  withLock(
    join(targetDir, 'perf-harness-build.lock'),
    () => {
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
      replaceCachedBinary({ from: join(targetDir, profile), cache, profile });
    },
    { say: (text) => console.error(`[perf-harness] ${text}`) }
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
