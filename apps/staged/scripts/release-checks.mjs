// Shared validation helpers for Staged's stable release pipeline.
//
// These checks enforce the stable update contract documented in
// docs/release-and-updates.md: a single stable channel, a fixed HTTPS
// credential-free latest.json endpoint, immutable staged/vX.Y.Z version tags,
// fixed asset names, and a valid darwin-aarch64 updater feed. Publication
// scripts fail closed on any violation rather than shipping a broken feed.

export const STABLE_CHANNEL = 'stable';

/** Only the stable channel is supported. Fail closed on anything else. */
export function assertStableChannel(channel) {
  if (channel !== STABLE_CHANNEL) {
    throw new Error(
      `Unsupported release channel "${channel}". Only "${STABLE_CHANNEL}" is supported.`
    );
  }
}

/** Versions must be plain X.Y.Z semver (no prerelease/build metadata). */
export function assertValidVersion(version) {
  if (typeof version !== 'string' || !/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(`Invalid release version "${version}". Expected X.Y.Z.`);
  }
}

/** The immutable version tag must be staged/vX.Y.Z for the released version. */
export function assertVersionTagMatchesVersion(tag, version) {
  const expected = `staged/v${version}`;
  if (tag !== expected) {
    throw new Error(
      `Release tag "${tag}" does not match app version "${version}" (expected "${expected}").`
    );
  }
}

/**
 * The updater endpoint must be HTTPS, credential-free (no userinfo, query, or
 * fragment), and point at a latest.json feed. This keeps the single stable
 * feed naming scheme and prevents tokens from leaking into the build config.
 */
export function assertValidUpdaterEndpoint(endpoint) {
  let url;
  try {
    url = new URL(endpoint);
  } catch {
    throw new Error(`Updater endpoint is not a valid URL: "${endpoint}"`);
  }
  if (url.protocol !== 'https:') {
    throw new Error(`Updater endpoint must use https, got "${endpoint}".`);
  }
  if (url.username || url.password) {
    throw new Error(`Updater endpoint must not embed credentials: "${endpoint}".`);
  }
  if (url.search || url.hash) {
    throw new Error(`Updater endpoint must not include a query string or fragment: "${endpoint}".`);
  }
  if (!url.pathname.endsWith('/latest.json')) {
    throw new Error(`Updater endpoint must point at a latest.json feed, got "${endpoint}".`);
  }
}

/** The updater public key is a single-line base64 string (Tauri signer format). */
export function assertValidUpdaterPublicKey(pubkey) {
  if (typeof pubkey !== 'string' || !pubkey.trim()) {
    throw new Error('Updater public key must be a non-empty string.');
  }
  if (!/^[A-Za-z0-9+/=]+$/.test(pubkey.trim())) {
    throw new Error(
      'Updater public key must be the single-line base64 value produced by `tauri signer generate`.'
    );
  }
}

/**
 * Validate a latest.json feed object for a stable release: the feed version
 * matches the released version, the required platform record is present, its
 * signature is nonempty, and its URL points at the expected stable-alias
 * archive asset.
 */
export function assertValidStableFeed(feed, { version, platform, expectedUrl }) {
  if (!feed || typeof feed !== 'object') {
    throw new Error('latest.json feed must be a JSON object.');
  }
  if (feed.version !== version) {
    throw new Error(
      `latest.json version "${feed.version}" does not match release version "${version}".`
    );
  }
  const platforms = feed.platforms;
  if (!platforms || typeof platforms !== 'object') {
    throw new Error('latest.json is missing the "platforms" object.');
  }
  const record = platforms[platform];
  if (!record || typeof record !== 'object') {
    const available = Object.keys(platforms);
    throw new Error(
      `latest.json is missing required platform "${platform}". Available: ${available.join(', ') || '(none)'}`
    );
  }
  if (typeof record.signature !== 'string' || !record.signature.trim()) {
    throw new Error(`latest.json platform "${platform}" has an empty signature.`);
  }
  if (record.url !== expectedUrl) {
    throw new Error(
      `latest.json platform "${platform}" URL "${record.url}" does not match the expected stable alias asset "${expectedUrl}".`
    );
  }
  assertValidHttpsAssetUrl(record.url);
}

/** Published asset URLs must be plain HTTPS without embedded credentials. */
export function assertValidHttpsAssetUrl(assetUrl) {
  let url;
  try {
    url = new URL(assetUrl);
  } catch {
    throw new Error(`Asset URL is not a valid URL: "${assetUrl}"`);
  }
  if (url.protocol !== 'https:') {
    throw new Error(`Asset URL must use https, got "${assetUrl}".`);
  }
  if (url.username || url.password) {
    throw new Error(`Asset URL must not embed credentials: "${assetUrl}".`);
  }
}

/**
 * Verify a published URL responds successfully. Tries HEAD first, then a
 * ranged GET for CDNs that reject HEAD. Retries to allow for propagation.
 */
export async function assertUrlReachable(
  url,
  { fetchImpl = fetch, attempts = 3, delayMs = 2000 } = {}
) {
  let lastFailure = 'unknown error';
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      const head = await fetchImpl(url, { method: 'HEAD', redirect: 'follow' });
      if (head.ok) return;
      const get = await fetchImpl(url, {
        method: 'GET',
        redirect: 'follow',
        headers: { Range: 'bytes=0-0' },
      });
      // Response.ok covers 200–299, including 206 Partial Content.
      if (get.ok) return;
      lastFailure = `HTTP ${head.status} (HEAD) / HTTP ${get.status} (GET)`;
    } catch (error) {
      lastFailure = error instanceof Error ? error.message : String(error);
    }
    if (attempt < attempts) {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, delayMs));
    }
  }
  throw new Error(`Published asset is not reachable: ${url} (${lastFailure})`);
}

/** Verify every published URL in order, failing on the first unreachable one. */
export async function assertUrlsReachable(urls, options = {}) {
  for (const url of urls) {
    await assertUrlReachable(url, options);
  }
}
