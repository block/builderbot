// Script-level tests for the release pipeline, exercised end-to-end via
// child processes against temporary dry-run fixtures. No test mutates real
// GitHub releases: the publish scripts run with DRY_RUN=1 and never invoke
// `gh`, and the config script writes into a disposable fixture directory.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(scriptsDir, '..');

const STABLE_ENDPOINT =
  'https://github.com/block/builderbot/releases/download/staged-latest/latest.json';
const PUBLIC_KEY = 'dW50cnVzdGVkIGNvbW1lbnQ6IHN0YWdlZCBwdWJrZXk=';
const VERSION = '0.1.9';

let fixtureDir: string;

function writeFixtureConfig(overrides: Record<string, unknown> = {}) {
  const baseConfig = JSON.parse(readFileSync(join(appDir, 'src-tauri/tauri.conf.json'), 'utf-8'));
  const config = { ...baseConfig, version: VERSION, ...overrides };
  mkdirSync(join(fixtureDir, 'src-tauri'), { recursive: true });
  writeFileSync(
    join(fixtureDir, 'src-tauri/tauri.conf.json'),
    `${JSON.stringify(config, null, 2)}\n`
  );
}

interface RunResult {
  status: number;
  stdout: string;
  stderr: string;
}

function runScript(
  script: string,
  { env = {}, args = [] }: { env?: Record<string, string>; args?: string[] } = {}
): RunResult {
  const scriptPath = join(scriptsDir, script);
  const cleanEnv: Record<string, string | undefined> = {
    ...process.env,
    STAGED_UPDATER_PUBLIC_KEY: undefined,
    STAGED_UPDATER_ENDPOINT: undefined,
    STAGED_OFFICIAL_RELEASE: undefined,
    STAGED_RELEASE_CHANNEL: undefined,
    STAGED_APPLE_SIGNING_IDENTITY: undefined,
    APPLE_SIGNING_IDENTITY: undefined,
    VERSION: undefined,
    RELEASE_TAG: undefined,
    ...env,
  };
  try {
    const stdout = execFileSync(process.execPath, [scriptPath, ...args], {
      cwd: fixtureDir,
      env: cleanEnv as NodeJS.ProcessEnv,
      encoding: 'utf-8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    return { status: 0, stdout, stderr: '' };
  } catch (error) {
    const failure = error as { status?: number; stdout?: string; stderr?: string };
    return {
      status: failure.status ?? 1,
      stdout: failure.stdout?.toString() ?? '',
      stderr: failure.stderr?.toString() ?? '',
    };
  }
}

function setupUpdaterBundle({
  signature = 'c2lnbmF0dXJl',
  includeArchive = true,
  includeSignature = true,
  latestJson,
}: {
  signature?: string;
  includeArchive?: boolean;
  includeSignature?: boolean;
  latestJson?: Record<string, unknown>;
} = {}) {
  const bundleDir = join(fixtureDir, 'src-tauri/target/aarch64-apple-darwin/release/bundle/macos');
  mkdirSync(bundleDir, { recursive: true });
  if (includeArchive) {
    writeFileSync(join(bundleDir, 'Staged.app.tar.gz'), 'archive-bytes');
  }
  if (includeSignature) {
    writeFileSync(join(bundleDir, 'Staged.app.tar.gz.sig'), signature);
  }
  if (latestJson) {
    writeFileSync(join(bundleDir, 'latest.json'), `${JSON.stringify(latestJson, null, 2)}\n`);
  }
  return bundleDir;
}

function setupDmgBundle() {
  const dmgDir = join(fixtureDir, 'src-tauri/target/aarch64-apple-darwin/release/bundle/dmg');
  mkdirSync(dmgDir, { recursive: true });
  const dmgPath = join(dmgDir, `Staged_${VERSION}_aarch64.dmg`);
  writeFileSync(dmgPath, 'dmg-bytes');
  return dmgPath;
}

beforeEach(() => {
  fixtureDir = mkdtempSync(join(tmpdir(), 'staged-release-scripts-'));
  writeFixtureConfig();
});

afterEach(() => {
  rmSync(fixtureDir, { recursive: true, force: true });
});

describe('build-tauri-release-config.mjs', () => {
  const releaseConfigPath = () => join(fixtureDir, 'src-tauri/tauri.release.conf.json');

  it('writes updater config with a valid key and endpoint', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: {
        STAGED_UPDATER_PUBLIC_KEY: PUBLIC_KEY,
        STAGED_UPDATER_ENDPOINT: STABLE_ENDPOINT,
      },
    });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('Release channel: stable');
    expect(result.stdout).toContain(`Updater config enabled (${STABLE_ENDPOINT})`);

    const config = JSON.parse(readFileSync(releaseConfigPath(), 'utf-8'));
    expect(config.bundle.createUpdaterArtifacts).toBe('v1Compatible');
    expect(config.plugins.updater).toEqual({
      pubkey: PUBLIC_KEY,
      endpoints: [STABLE_ENDPOINT],
    });
  });

  it('writes the trimmed key and endpoint when the secrets carry a trailing newline', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: {
        STAGED_UPDATER_PUBLIC_KEY: `${PUBLIC_KEY}\n`,
        STAGED_UPDATER_ENDPOINT: `${STABLE_ENDPOINT}\n`,
      },
    });
    expect(result.status).toBe(0);

    // Tauri's base64 decoder rejects an untrimmed key at runtime, so the value
    // written to the config must match the validated one exactly.
    const config = JSON.parse(readFileSync(releaseConfigPath(), 'utf-8'));
    expect(config.plugins.updater).toEqual({
      pubkey: PUBLIC_KEY,
      endpoints: [STABLE_ENDPOINT],
    });
  });

  it('rejects non-stable channels', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: { STAGED_RELEASE_CHANNEL: 'beta' },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Only "stable" is supported');
    expect(existsSync(releaseConfigPath())).toBe(false);
  });

  it('fails closed for official builds when updater credentials are missing', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: { STAGED_OFFICIAL_RELEASE: 'true' },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Official release builds require updater credentials');
    expect(result.stderr).toContain('STAGED_UPDATER_PUBLIC_KEY');
    expect(result.stderr).toContain('STAGED_UPDATER_ENDPOINT');
    expect(existsSync(releaseConfigPath())).toBe(false);
  });

  it('fails closed for official builds when only the endpoint is missing', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: {
        STAGED_OFFICIAL_RELEASE: 'true',
        STAGED_UPDATER_PUBLIC_KEY: PUBLIC_KEY,
      },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('STAGED_UPDATER_ENDPOINT');
    expect(result.stderr).not.toContain('STAGED_UPDATER_PUBLIC_KEY,');
  });

  it('rejects a non-https endpoint', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: {
        STAGED_UPDATER_PUBLIC_KEY: PUBLIC_KEY,
        STAGED_UPDATER_ENDPOINT: STABLE_ENDPOINT.replace('https://', 'http://'),
      },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('must use https');
  });

  it('rejects an endpoint with embedded credentials', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: {
        STAGED_UPDATER_PUBLIC_KEY: PUBLIC_KEY,
        STAGED_UPDATER_ENDPOINT:
          'https://user:token@github.com/block/builderbot/releases/download/staged-latest/latest.json',
      },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('must not embed credentials');
  });

  it('rejects an invalid public key', () => {
    const result = runScript('build-tauri-release-config.mjs', {
      env: {
        STAGED_UPDATER_PUBLIC_KEY: 'not base64!!',
        STAGED_UPDATER_ENDPOINT: STABLE_ENDPOINT,
      },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('base64');
  });

  it('still writes a config without updater settings for unofficial builds', () => {
    const result = runScript('build-tauri-release-config.mjs');
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('Updater config skipped');

    const config = JSON.parse(readFileSync(releaseConfigPath(), 'utf-8'));
    expect(config.bundle.createUpdaterArtifacts).toBe('v1Compatible');
    expect(config.plugins?.updater).toBeUndefined();
  });
});

describe('publish-updater-to-github-release.mjs', () => {
  const dryEnv = { DRY_RUN: '1' };

  it('dry-run validates the feed and prints upload commands without running gh', () => {
    setupUpdaterBundle();
    const result = runScript('publish-updater-to-github-release.mjs', { env: dryEnv });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('Release channel: stable');
    expect(result.stdout).toContain('Feed validation passed');
    expect(result.stdout).toContain('DRY_RUN enabled. Skipping upload.');
    expect(result.stdout).toContain('gh release upload staged-latest');
    expect(result.stdout).toContain(
      'https://github.com/block/builderbot/releases/download/staged-latest/latest.json'
    );
  });

  it('rejects non-stable channels', () => {
    setupUpdaterBundle();
    const result = runScript('publish-updater-to-github-release.mjs', {
      env: { ...dryEnv, STAGED_RELEASE_CHANNEL: 'beta' },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Only "stable" is supported');
  });

  it('rejects a release tag that does not match the app version', () => {
    setupUpdaterBundle();
    const result = runScript('publish-updater-to-github-release.mjs', {
      env: { ...dryEnv, RELEASE_TAG: 'staged/v9.9.9' },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('does not match app version');
  });

  it('rejects an invalid version', () => {
    setupUpdaterBundle();
    const result = runScript('publish-updater-to-github-release.mjs', {
      env: { ...dryEnv, VERSION: '1.2.3-beta.1', RELEASE_TAG: 'staged/v1.2.3-beta.1' },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Invalid release version');
  });

  it('fails when the updater archive is missing', () => {
    setupUpdaterBundle({ includeArchive: false });
    const result = runScript('publish-updater-to-github-release.mjs', { env: dryEnv });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Could not find updater archive');
  });

  it('fails when the signature file is missing', () => {
    setupUpdaterBundle({ includeSignature: false });
    const result = runScript('publish-updater-to-github-release.mjs', { env: dryEnv });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Missing required file');
    expect(result.stderr).toContain('Staged.app.tar.gz.sig');
  });

  it('fails when the signature file is empty', () => {
    setupUpdaterBundle({ signature: '   ' });
    const result = runScript('publish-updater-to-github-release.mjs', { env: dryEnv });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('signature');
  });

  it('fails when latest.json lacks the darwin-aarch64 platform', () => {
    setupUpdaterBundle({
      latestJson: {
        version: VERSION,
        platforms: {
          'linux-x86_64': { signature: 'c2ln', url: '' },
        },
      },
    });
    const result = runScript('publish-updater-to-github-release.mjs', { env: dryEnv });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Platform "darwin-aarch64" missing in latest.json');
  });

  it('fails when the generated latest.json version does not match the release version', () => {
    setupUpdaterBundle({
      latestJson: {
        version: '0.0.1',
        platforms: {
          'darwin-aarch64': { signature: 'stale', url: 'https://example.com/old.tar.gz' },
        },
      },
    });
    const result = runScript('publish-updater-to-github-release.mjs', { env: dryEnv });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain(
      `latest.json version "0.0.1" does not match release version "${VERSION}"`
    );
  });

  it('rewrites the feed URL and signature to the stable alias before validating', () => {
    setupUpdaterBundle({
      latestJson: {
        version: VERSION,
        platforms: {
          'darwin-aarch64': { signature: 'stale', url: 'https://example.com/old.tar.gz' },
        },
      },
    });
    const result = runScript('publish-updater-to-github-release.mjs', { env: dryEnv });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('Feed validation passed');
  });
});

describe('publish-dmg-to-github-release.mjs', () => {
  const dryEnv = { DRY_RUN: '1' };

  it('dry-run prints versioned and alias upload commands without running gh', () => {
    setupDmgBundle();
    const result = runScript('publish-dmg-to-github-release.mjs', { env: dryEnv });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('Release channel: stable');
    expect(result.stdout).toContain('DRY_RUN enabled. Skipping upload.');
    expect(result.stdout).toContain(`gh release upload staged/v${VERSION}`);
    expect(result.stdout).toContain('gh release upload staged-latest');
    expect(result.stdout).toContain(
      `Versioned URL: https://github.com/block/builderbot/releases/download/staged/v${VERSION}/Staged_${VERSION}_aarch64.dmg`
    );
    expect(result.stdout).toContain(
      'Channel URL:   https://github.com/block/builderbot/releases/download/staged-latest/Staged-latest-aarch64.dmg'
    );
  });

  it('rejects non-stable channels', () => {
    setupDmgBundle();
    const result = runScript('publish-dmg-to-github-release.mjs', {
      env: { ...dryEnv, STAGED_RELEASE_CHANNEL: 'nightly' },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Only "stable" is supported');
  });

  it('rejects a release tag that does not match the app version', () => {
    setupDmgBundle();
    const result = runScript('publish-dmg-to-github-release.mjs', {
      env: { ...dryEnv, RELEASE_TAG: 'staged/v0.0.1' },
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('does not match app version');
  });

  it('fails when the DMG artifact is missing', () => {
    const result = runScript('publish-dmg-to-github-release.mjs', { env: dryEnv });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Missing DMG artifact');
  });
});
