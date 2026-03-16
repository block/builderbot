import { execFileSync } from 'node:child_process';
import { basename, join, resolve } from 'node:path';
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';

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
const version = process.env.VERSION ?? readVersionFromConfig();
const versionTag = process.env.RELEASE_TAG ?? `staged/v${version}`;
const latestTag = process.env.CHANNEL_LATEST_TAG ?? 'staged-latest';
const releaseTarget = process.env.RELEASE_TARGET;
const versionReleaseTitle = process.env.VERSION_RELEASE_TITLE ?? `Staged v${version}`;
const latestReleaseTitle = process.env.LATEST_RELEASE_TITLE ?? 'Staged Stable Latest';
const dmgProductName = process.env.DMG_PRODUCT_NAME ?? 'Staged';
const latestAssetName = process.env.LATEST_DMG_ASSET_NAME ?? 'Staged-latest-aarch64.dmg';
const dryRun = process.env.DRY_RUN === 'true' || process.env.DRY_RUN === '1';

const dmgPath = resolve(
  process.cwd(),
  process.env.DMG_PATH ??
    `src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/${dmgProductName}_${version}_aarch64.dmg`,
);

function readVersionFromConfig() {
  const configPath = resolve(process.cwd(), 'src-tauri/tauri.conf.json');
  const config = JSON.parse(readFileSync(configPath, 'utf-8'));
  const configVersion = config?.version;
  if (typeof configVersion !== 'string' || !configVersion.trim()) {
    throw new Error(`Could not determine version from ${configPath}`);
  }
  return configVersion;
}

function quote(arg) {
  if (/^[a-zA-Z0-9._:/=-]+$/.test(arg)) return arg;
  return `'${arg.replace(/'/g, `'\\''`)}'`;
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

function ensureRelease(tag, { title, prerelease }) {
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
  if (prerelease) {
    args.push('--prerelease');
  }
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

function versionedAssetUrl(tag, name) {
  return `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(name)}`;
}

function main() {
  if (!existsSync(dmgPath)) {
    throw new Error(`Missing DMG artifact: ${dmgPath}`);
  }

  const stageDir = mkdtempSync(join(tmpdir(), 'staged-release-dmg-'));
  const latestAliasPath = join(stageDir, latestAssetName);
  const dmgName = basename(dmgPath);

  try {
    cpSync(dmgPath, latestAliasPath);

    const uploadVersionedArgs = ['release', 'upload', versionTag, dmgPath, '--repo', repo, '--clobber'];
    const uploadLatestArgs = [
      'release',
      'upload',
      latestTag,
      latestAliasPath,
      '--repo',
      repo,
      '--clobber',
    ];

    console.log('Release channel: stable');
    console.log(`Preparing DMG upload for ${repo}`);
    console.log(`- version tag:    ${versionTag}`);
    console.log(`- latest tag:     ${latestTag}`);
    console.log(`- versioned asset: ${dmgName}`);
    console.log(`- latest alias:   ${latestAssetName}`);

    if (dryRun) {
      console.log('DRY_RUN enabled. Skipping upload.');
      console.log(`gh release view ${quote(versionTag)} --repo ${quote(repo)}`);
      console.log(
        `gh release view ${quote(latestTag)} --repo ${quote(repo)} || gh release create ${quote(latestTag)} --repo ${quote(repo)} --title ${quote(latestReleaseTitle)} --notes ${quote('Automated stable release placeholder.')}${releaseTarget ? ` --target ${quote(releaseTarget)}` : ''}`,
      );
      console.log(`gh ${uploadVersionedArgs.map(quote).join(' ')}`);
      console.log(`gh ${uploadLatestArgs.map(quote).join(' ')}`);
      console.log(`Versioned URL: ${versionedAssetUrl(versionTag, dmgName)}`);
      console.log(`Channel URL:   ${versionedAssetUrl(latestTag, latestAssetName)}`);
      return;
    }

    ensureRelease(versionTag, { title: versionReleaseTitle, prerelease: false });
    ensureRelease(latestTag, { title: latestReleaseTitle, prerelease: false });

    runGh(uploadVersionedArgs, { stdio: 'inherit' });
    runGh(uploadLatestArgs, { stdio: 'inherit' });

    const versionAssets = readReleaseAssets(versionTag);
    const latestAssets = readReleaseAssets(latestTag);
    if (!versionAssets.includes(dmgName)) {
      throw new Error(`Release ${versionTag} is missing versioned asset ${dmgName} after upload`);
    }
    if (!latestAssets.includes(latestAssetName)) {
      throw new Error(`Release ${latestTag} is missing latest alias asset ${latestAssetName} after upload`);
    }

    console.log('GitHub DMG assets verified.');
    console.log(`Versioned URL: ${versionedAssetUrl(versionTag, dmgName)}`);
    console.log(`Channel URL:   ${versionedAssetUrl(latestTag, latestAssetName)}`);
  } finally {
    rmSync(stageDir, { recursive: true, force: true });
  }
}

main();
