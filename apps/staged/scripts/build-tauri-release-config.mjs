// Generates the gitignored src-tauri/tauri.release.conf.json for official
// stable release builds. Enforces the stable update contract documented in
// docs/release-and-updates.md:
//
// - stable-only channel
// - HTTPS, credential-free updater endpoint pointing at latest.json
// - updater key and endpoint both required for official builds (fail closed)
// - bundle.createUpdaterArtifacts stays "v1Compatible"
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  assertStableChannel,
  normalizeUpdaterEndpoint,
  normalizeUpdaterPublicKey,
} from './release-checks.mjs';

function readArg(name) {
  const exact = `--${name}`;
  const prefix = `--${name}=`;
  for (let index = 2; index < process.argv.length; index += 1) {
    const current = process.argv[index];
    if (current === exact) {
      return process.argv[index + 1];
    }
    if (current.startsWith(prefix)) {
      return current.slice(prefix.length);
    }
  }
  return undefined;
}

function hasFlag(name) {
  return process.argv.slice(2).includes(`--${name}`);
}

const requestedChannel = (
  readArg('channel') ??
  process.env.STAGED_RELEASE_CHANNEL ??
  'stable'
).toLowerCase();
assertStableChannel(requestedChannel);

// Official release builds (CI or --official) must fail closed rather than
// silently producing a release configuration without updater settings.
const officialRelease =
  hasFlag('official') ||
  process.env.STAGED_OFFICIAL_RELEASE === 'true' ||
  process.env.STAGED_OFFICIAL_RELEASE === '1';

const publicKey = process.env.STAGED_UPDATER_PUBLIC_KEY;
const endpoint = process.env.STAGED_UPDATER_ENDPOINT;
const signingIdentity =
  process.env.STAGED_APPLE_SIGNING_IDENTITY ?? process.env.APPLE_SIGNING_IDENTITY;

const missing = [];
if (!publicKey) missing.push('STAGED_UPDATER_PUBLIC_KEY');
if (!endpoint) missing.push('STAGED_UPDATER_ENDPOINT');

if (officialRelease && missing.length > 0) {
  throw new Error(
    `Official release builds require updater credentials. Missing: ${missing.join(', ')}`
  );
}

// Validation returns the value that gets written, so the config can never
// contain an unvalidated variant (e.g. a key with a trailing newline that
// Tauri's base64 decoder would reject at runtime).
const normalizedPublicKey = publicKey ? normalizeUpdaterPublicKey(publicKey) : undefined;
const normalizedEndpoint = endpoint ? normalizeUpdaterEndpoint(endpoint) : undefined;

const baseConfigPath = resolve(process.cwd(), 'src-tauri/tauri.conf.json');
const outputConfigPath = resolve(process.cwd(), 'src-tauri/tauri.release.conf.json');
const baseConfig = JSON.parse(readFileSync(baseConfigPath, 'utf-8'));

const releaseConfig = { ...baseConfig };

console.log('Release channel: stable');
if (officialRelease) {
  console.log('Official release mode: updater configuration is required.');
}

releaseConfig.bundle = {
  ...(releaseConfig.bundle ?? baseConfig.bundle ?? {}),
  createUpdaterArtifacts: 'v1Compatible',
};

if (signingIdentity) {
  releaseConfig.bundle.macOS = {
    ...(releaseConfig.bundle?.macOS ?? baseConfig.bundle?.macOS ?? {}),
    signingIdentity,
  };
  console.log(`macOS signing identity configured (${signingIdentity})`);
}

if (normalizedPublicKey && normalizedEndpoint) {
  releaseConfig.plugins = {
    ...(baseConfig.plugins ?? {}),
    updater: {
      pubkey: normalizedPublicKey,
      endpoints: [normalizedEndpoint],
    },
  };
  console.log(`Updater config enabled (${normalizedEndpoint})`);
} else {
  console.log(`Updater config skipped (missing: ${missing.join(', ')})`);
}

writeFileSync(outputConfigPath, `${JSON.stringify(releaseConfig, null, 2)}\n`);
console.log(`Wrote ${outputConfigPath}`);
