// Publishes the stable updater feed and archive to the rolling staged-latest
// GitHub release alias. Enforces the stable update contract documented in
// docs/release-and-updates.md before and after upload:
//
// - stable-only channel, X.Y.Z version matching the staged/vX.Y.Z tag
// - archive and signature assets present with a nonempty signature
// - latest.json contains a valid darwin-aarch64 entry pointing at the
//   stable alias archive URL
// - published alias assets are reachable before the workflow reports success
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
import {
  assertStableChannel,
  assertValidStableFeed,
  assertValidVersion,
  assertVersionTagMatchesVersion,
  assertUrlsReachable,
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

const requestedChannel = (
  readArg('channel') ??
  process.env.STAGED_RELEASE_CHANNEL ??
  'stable'
).toLowerCase();
assertStableChannel(requestedChannel);

const repo = process.env.GITHUB_REPOSITORY ?? 'block/builderbot';
const tauriConfigPath = resolve(process.cwd(), 'src-tauri/tauri.conf.json');
const version = process.env.VERSION ?? readVersionFromConfig();
const versionTag = process.env.RELEASE_TAG ?? `staged/v${version}`;
const latestTag = process.env.CHANNEL_LATEST_TAG ?? 'staged-latest';
const latestReleaseTitle = process.env.LATEST_RELEASE_TITLE ?? 'Staged Stable Latest';
const releaseTarget = process.env.RELEASE_TARGET;
const tauriTarget = process.env.TAURI_TARGET ?? 'aarch64-apple-darwin';
const updaterPlatform = process.env.UPDATER_PLATFORM ?? 'darwin-aarch64';
const updaterArchiveName = process.env.UPDATER_ARCHIVE_NAME;
const productName = process.env.UPDATER_PRODUCT_NAME ?? 'Staged';
const dryRun = process.env.DRY_RUN === 'true' || process.env.DRY_RUN === '1';
const skipReachabilityCheck =
  process.env.SKIP_REACHABILITY_CHECK === 'true' || process.env.SKIP_REACHABILITY_CHECK === '1';
const defaultBundleDirs = [
  `src-tauri/target/${tauriTarget}/release/bundle/macos`,
  'src-tauri/target/release/bundle/macos',
];
const bundleDir = resolve(
  process.cwd(),
  process.env.UPDATER_BUNDLE_DIR ??
    defaultBundleDirs.find((dir) => existsSync(resolve(process.cwd(), dir))) ??
    defaultBundleDirs[1]
);
const latestPath = join(bundleDir, 'latest.json');

assertValidVersion(version);
assertVersionTagMatchesVersion(versionTag, version);

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
    { encoding: 'utf-8' }
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
      `Could not find updater archive in ${bundleDir}. Expected ${canonicalArchiveName} or set UPDATER_ARCHIVE_NAME.`
    );
  }
  throw new Error(
    `Found multiple updater archives in ${bundleDir}: ${candidates.join(', ')}. Set UPDATER_ARCHIVE_NAME.`
  );
}

function readSignature(signaturePath) {
  const signature = readFileSync(signaturePath, 'utf-8').trim();
  if (!signature) {
    throw new Error(`Updater signature file is empty: ${signaturePath}`);
  }
  return signature;
}

function buildStableLatest(signaturePath) {
  return {
    version,
    notes: `Release v${version}.`,
    pub_date: new Date().toISOString(),
    platforms: {
      [updaterPlatform]: {
        signature: readSignature(signaturePath),
        url: '',
      },
    },
  };
}

function downloadUrl(name) {
  return `https://github.com/${repo}/releases/download/${latestTag}/${encodeURIComponent(name)}`;
}

async function main() {
  const archivePath = resolveArchivePath();
  const signaturePath = `${archivePath}.sig`;
  requirePath(archivePath);
  requirePath(signaturePath);

  const latest = existsSync(latestPath)
    ? JSON.parse(readFileSync(latestPath, 'utf-8'))
    : buildStableLatest(signaturePath);
  // The Tauri-generated feed version must already match the release version:
  // a mismatch means the tag was bumped without rebuilding the bundle, and
  // rewriting it here would advertise a version the archive does not contain.
  if (latest.version !== version) {
    throw new Error(
      `latest.json version "${latest.version}" does not match release version "${version}". ` +
        'The bundle was likely built from a different version; rebuild before publishing.'
    );
  }
  latest.pub_date = new Date().toISOString();
  const platformRecord = latest?.platforms?.[updaterPlatform];
  if (!platformRecord) {
    const available = Object.keys(latest?.platforms ?? {});
    throw new Error(
      `Platform "${updaterPlatform}" missing in latest.json. Available: ${available.join(', ') || '(none)'}`
    );
  }

  const archiveName = basename(archivePath);
  const signatureName = basename(signaturePath);
  platformRecord.signature = readSignature(signaturePath);
  platformRecord.url = downloadUrl(archiveName);

  // Validate the generated feed before publishing anything to the alias.
  assertValidStableFeed(latest, {
    version,
    platform: updaterPlatform,
    expectedUrl: downloadUrl(archiveName),
  });

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
    console.log('Feed validation passed (version, platform, signature, URL).');

    if (dryRun) {
      console.log('DRY_RUN enabled. Skipping upload.');
      console.log(
        `gh release view ${latestTag} --repo ${repo} || gh release create ${latestTag} --repo ${repo} --title "${latestReleaseTitle}" --notes "Automated stable release placeholder."`
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

    if (skipReachabilityCheck) {
      console.log('SKIP_REACHABILITY_CHECK enabled. Skipping published URL checks.');
    } else {
      const publishedUrls = [
        downloadUrl('latest.json'),
        downloadUrl(archiveName),
        downloadUrl(signatureName),
      ];
      console.log('Verifying published stable alias assets are reachable...');
      await assertUrlsReachable(publishedUrls);
    }

    console.log('GitHub updater alias assets verified.');
    console.log(`Updater endpoint: ${downloadUrl('latest.json')}`);
  } finally {
    rmSync(stageDir, { recursive: true, force: true });
  }
}

await main();
