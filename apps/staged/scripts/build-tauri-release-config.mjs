import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

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

const requestedChannel = (
  readArg('channel') ?? process.env.STAGED_RELEASE_CHANNEL ?? 'stable'
).toLowerCase();
if (requestedChannel !== 'stable') {
  throw new Error(
    `Unsupported release channel "${requestedChannel}". Only "stable" is supported.`,
  );
}

const publicKey = process.env.STAGED_UPDATER_PUBLIC_KEY;
const endpoint = process.env.STAGED_UPDATER_ENDPOINT;
const signingIdentity =
  process.env.STAGED_APPLE_SIGNING_IDENTITY ?? process.env.APPLE_SIGNING_IDENTITY;

const baseConfigPath = resolve(process.cwd(), 'src-tauri/tauri.conf.json');
const outputConfigPath = resolve(process.cwd(), 'src-tauri/tauri.release.conf.json');
const baseConfig = JSON.parse(readFileSync(baseConfigPath, 'utf-8'));

const releaseConfig = { ...baseConfig };

console.log('Release channel: stable');

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

if (publicKey && endpoint) {
  releaseConfig.plugins = {
    ...(baseConfig.plugins ?? {}),
    updater: {
      pubkey: publicKey,
      endpoints: [endpoint],
    },
  };
  console.log(`Updater config enabled (${endpoint})`);
} else {
  const missing = [];
  if (!publicKey) missing.push('STAGED_UPDATER_PUBLIC_KEY');
  if (!endpoint) missing.push('STAGED_UPDATER_ENDPOINT');
  console.log(`Updater config skipped (missing: ${missing.join(', ')})`);
}

writeFileSync(outputConfigPath, `${JSON.stringify(releaseConfig, null, 2)}\n`);
console.log(`Wrote ${outputConfigPath}`);
