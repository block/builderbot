import { execFileSync } from 'node:child_process';
import {
  cpSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';

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

const repo = process.env.GITHUB_REPOSITORY ?? 'block/builderbot';
const tauriConfigPath = resolve(process.cwd(), 'src-tauri/tauri.conf.json');
const version = process.env.VERSION ?? readVersionFromConfig();
const latestTag = process.env.CHANNEL_LATEST_TAG ?? 'staged-latest';
const latestReleaseTitle = process.env.LATEST_RELEASE_TITLE ?? 'Staged Stable Latest';
const releaseTarget = process.env.RELEASE_TARGET;
const tauriTarget = process.env.TAURI_TARGET ?? 'aarch64-apple-darwin';
const updaterPlatform = process.env.UPDATER_PLATFORM ?? 'darwin-aarch64';
const updaterArchiveName = process.env.UPDATER_ARCHIVE_NAME;
const productName = process.env.UPDATER_PRODUCT_NAME ?? 'Staged';
const dryRun = process.env.DRY_RUN === 'true' || process.env.DRY_RUN === '1';
const defaultBundleDirs = [
  `src-tauri/target/${tauriTarget}/release/bundle/macos`,
  'src-tauri/target/release/bundle/macos',
];
const bundleDir = resolve(
  process.cwd(),
  process.env.UPDATER_BUNDLE_DIR ??
    defaultBundleDirs.find((dir) => existsSync(resolve(process.cwd(), dir))) ??
    defaultBundleDirs[1],
);
const latestPath = join(bundleDir, 'latest.json');

function readVersionFromConfig() {
  const config = JSON.parse(readFileSync(tauriConfigPath, 'utf-8'));
  const configVersion = config?.version;
  if (typeof configVersion !== 'string' || !configVersion.trim()) {
    throw new Error(`Could not determine version from ${tauriConfigPath}`);
  }
  return configVersion;
}

function requirePath(path) {
  if (!existsSync(path)) {
    throw new Error(`Missing required file: ${path}`);
  }
}

function runGh(args, options = {}) {
  return execFileSync('gh', args, options);
}

function releaseExists(tag) {
  try {
    runGh(['release', 'view', tag, '--repo', repo], { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

function ensureRelease(tag, title) {
  if (releaseExists(tag)) {
    return;
  }

  const args = [
    'release',
    'create',
    tag,
    '--repo',
    repo,
    '--title',
    title,
    '--notes',
    'Automated stable release placeholder.',
  ];
  if (releaseTarget) {
    args.push('--target', releaseTarget);
  }

  runGh(args, { stdio: 'inherit' });
}

function readReleaseAssets(tag) {
  const assetsRaw = runGh(
    ['release', 'view', tag, '--repo', repo, '--json', 'assets', '--jq', '.assets[].name'],
    { encoding: 'utf-8' },
  );
  return assetsRaw
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function resolveArchivePath() {
  if (updaterArchiveName) {
    const explicitPath = join(bundleDir, updaterArchiveName);
    requirePath(explicitPath);
    return explicitPath;
  }

  const canonicalArchiveName = `${productName}.app.tar.gz`;
  const canonicalPath = join(bundleDir, canonicalArchiveName);
  if (existsSync(canonicalPath)) {
    return canonicalPath;
  }

  const candidates = readdirSync(bundleDir).filter((entry) => entry.endsWith('.app.tar.gz'));
  if (candidates.length === 1) {
    return join(bundleDir, candidates[0]);
  }
  if (candidates.length === 0) {
    throw new Error(
      `Could not find updater archive in ${bundleDir}. Expected ${canonicalArchiveName} or set UPDATER_ARCHIVE_NAME.`,
    );
  }
  throw new Error(
    `Found multiple updater archives in ${bundleDir}: ${candidates.join(', ')}. Set UPDATER_ARCHIVE_NAME.`,
  );
}

function buildStableLatest(signaturePath) {
  const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, 'utf-8'));
  const signature = readFileSync(signaturePath, 'utf-8').trim();
  return {
    version: tauriConfig.version,
    notes: `Release v${tauriConfig.version}.`,
    pub_date: new Date().toISOString(),
    platforms: {
      [updaterPlatform]: {
        signature,
        url: '',
      },
    },
  };
}

function downloadUrl(name) {
  return `https://github.com/${repo}/releases/download/${latestTag}/${encodeURIComponent(name)}`;
}

function main() {
  const archivePath = resolveArchivePath();
  const signaturePath = `${archivePath}.sig`;
  requirePath(archivePath);
  requirePath(signaturePath);

  const latest = existsSync(latestPath)
    ? JSON.parse(readFileSync(latestPath, 'utf-8'))
    : buildStableLatest(signaturePath);
  latest.version = version;
  latest.pub_date = new Date().toISOString();
  const platformRecord = latest?.platforms?.[updaterPlatform];
  if (!platformRecord) {
    const available = Object.keys(latest?.platforms ?? {});
    throw new Error(
      `Platform "${updaterPlatform}" missing in latest.json. Available: ${available.join(', ') || '(none)'}`,
    );
  }

  const archiveName = basename(archivePath);
  const signatureName = basename(signaturePath);
  platformRecord.signature = readFileSync(signaturePath, 'utf-8').trim();
  platformRecord.url = downloadUrl(archiveName);

  const stageDir = mkdtempSync(join(tmpdir(), 'staged-github-updater-'));
  const stagedLatestPath = join(stageDir, 'latest.json');

  try {
    cpSync(archivePath, join(stageDir, archiveName));
    cpSync(signaturePath, join(stageDir, signatureName));
    writeFileSync(stagedLatestPath, `${JSON.stringify(latest, null, 2)}\n`);

    const uploadArgs = [
      'release',
      'upload',
      latestTag,
      stagedLatestPath,
      join(stageDir, archiveName),
      join(stageDir, signatureName),
      '--repo',
      repo,
      '--clobber',
    ];

    console.log('Release channel: stable');
    console.log(`Preparing updater alias release for ${repo}`);
    console.log(`- latest tag:      ${latestTag}`);
    console.log(`- updater archive: ${archiveName}`);
    console.log(`- updater endpoint: ${downloadUrl('latest.json')}`);

    if (dryRun) {
      console.log('DRY_RUN enabled. Skipping upload.');
      console.log(
        `gh release view ${latestTag} --repo ${repo} || gh release create ${latestTag} --repo ${repo} --title "${latestReleaseTitle}" --notes "Automated stable release placeholder."`,
      );
      console.log(`gh ${uploadArgs.join(' ')}`);
      return;
    }

    ensureRelease(latestTag, latestReleaseTitle);
    runGh(uploadArgs, { stdio: 'inherit' });

    const latestAssets = readReleaseAssets(latestTag);
    for (const expected of ['latest.json', archiveName, signatureName]) {
      if (!latestAssets.includes(expected)) {
        throw new Error(`Release ${latestTag} is missing ${expected} after upload`);
      }
    }

    console.log('GitHub updater alias assets verified.');
    console.log(`Updater endpoint: ${downloadUrl('latest.json')}`);
  } finally {
    rmSync(stageDir, { recursive: true, force: true });
  }
}

main();
