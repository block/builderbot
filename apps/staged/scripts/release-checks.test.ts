import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { infoPlist, writeAppArchive } from './app-archive-fixture';
import {
  assertArchiveEmbedsVersion,
  assertStableChannel,
  assertValidHttpsAssetUrl,
  assertValidStableFeed,
  assertValidUpdaterEndpoint,
  assertValidUpdaterPublicKey,
  assertValidVersion,
  assertVersionTagMatchesVersion,
  assertUrlReachable,
  normalizeUpdaterEndpoint,
  normalizeUpdaterPublicKey,
} from './release-checks.mjs';

const STABLE_ENDPOINT =
  'https://github.com/block/builderbot/releases/download/staged-latest/latest.json';
const ARCHIVE_URL =
  'https://github.com/block/builderbot/releases/download/staged-latest/Staged.app.tar.gz';

describe('assertStableChannel', () => {
  it('accepts stable', () => {
    expect(() => assertStableChannel('stable')).not.toThrow();
  });

  it.each(['beta', 'main', 'nightly', '', 'STABLE '])('rejects %j', (channel) => {
    expect(() => assertStableChannel(channel)).toThrow(/Only "stable" is supported/);
  });
});

describe('assertValidVersion', () => {
  it('accepts plain X.Y.Z versions', () => {
    expect(() => assertValidVersion('0.1.9')).not.toThrow();
    expect(() => assertValidVersion('12.0.3')).not.toThrow();
  });

  it.each(['1.2', '1.2.3-beta.1', 'v1.2.3', '1.2.3.4', '', undefined])('rejects %j', (version) => {
    expect(() => assertValidVersion(version as string)).toThrow(/Invalid release version/);
  });
});

describe('assertVersionTagMatchesVersion', () => {
  it('accepts the immutable staged tag for the version', () => {
    expect(() => assertVersionTagMatchesVersion('staged/v0.1.9', '0.1.9')).not.toThrow();
  });

  it('rejects a tag for a different version', () => {
    expect(() => assertVersionTagMatchesVersion('staged/v0.1.8', '0.1.9')).toThrow(
      /does not match app version/
    );
  });

  it('rejects a tag from another naming scheme', () => {
    expect(() => assertVersionTagMatchesVersion('v0.1.9', '0.1.9')).toThrow(
      /does not match app version/
    );
  });
});

describe('assertValidUpdaterEndpoint', () => {
  it('accepts the fixed stable feed URL', () => {
    expect(() => assertValidUpdaterEndpoint(STABLE_ENDPOINT)).not.toThrow();
  });

  it('rejects plain http', () => {
    expect(() =>
      assertValidUpdaterEndpoint(STABLE_ENDPOINT.replace('https://', 'http://'))
    ).toThrow(/must use https/);
  });

  it('rejects embedded credentials', () => {
    expect(() =>
      assertValidUpdaterEndpoint(
        'https://user:token@github.com/block/builderbot/releases/download/staged-latest/latest.json'
      )
    ).toThrow(/must not embed credentials/);
  });

  it('rejects query strings and fragments', () => {
    expect(() => assertValidUpdaterEndpoint(`${STABLE_ENDPOINT}?token=abc`)).toThrow(
      /query string or fragment/
    );
    expect(() => assertValidUpdaterEndpoint(`${STABLE_ENDPOINT}#frag`)).toThrow(
      /query string or fragment/
    );
  });

  it('rejects endpoints that are not a latest.json feed', () => {
    expect(() =>
      assertValidUpdaterEndpoint('https://github.com/block/builderbot/releases/latest')
    ).toThrow(/latest\.json feed/);
  });

  it('rejects malformed URLs', () => {
    expect(() => assertValidUpdaterEndpoint('not a url')).toThrow(/not a valid URL/);
  });
});

describe('assertValidUpdaterPublicKey', () => {
  it('accepts a single-line base64 key', () => {
    expect(() => assertValidUpdaterPublicKey('dW50cnVzdGVkIGNvbW1lbnQ6IHB1YmtleQ==')).not.toThrow();
  });

  it('rejects empty or whitespace keys', () => {
    expect(() => assertValidUpdaterPublicKey('')).toThrow(/non-empty/);
    expect(() => assertValidUpdaterPublicKey('   ')).toThrow(/non-empty/);
  });

  it('rejects multi-line or non-base64 values', () => {
    expect(() => assertValidUpdaterPublicKey('line one\nline two')).toThrow(/base64/);
    expect(() => assertValidUpdaterPublicKey('has spaces inside')).toThrow(/base64/);
  });
});

describe('normalizeUpdaterPublicKey', () => {
  it('strips surrounding whitespace so the written key is the validated key', () => {
    expect(normalizeUpdaterPublicKey('  dW50cnVzdGVkIGNvbW1lbnQ6IHB1YmtleQ==\n')).toBe(
      'dW50cnVzdGVkIGNvbW1lbnQ6IHB1YmtleQ=='
    );
  });

  it('still rejects invalid keys', () => {
    expect(() => normalizeUpdaterPublicKey('not base64!!')).toThrow(/base64/);
    expect(() => normalizeUpdaterPublicKey('  ')).toThrow(/non-empty/);
  });
});

describe('normalizeUpdaterEndpoint', () => {
  it('strips surrounding whitespace so the written endpoint is the validated one', () => {
    expect(normalizeUpdaterEndpoint(`\n${STABLE_ENDPOINT}  `)).toBe(STABLE_ENDPOINT);
  });

  it('still rejects invalid endpoints', () => {
    expect(() => normalizeUpdaterEndpoint(` ${STABLE_ENDPOINT.replace('https', 'http')} `)).toThrow(
      /must use https/
    );
    expect(() => normalizeUpdaterEndpoint(undefined as unknown as string)).toThrow(
      /not a valid URL/
    );
  });
});

describe('assertValidStableFeed', () => {
  const validFeed = () => ({
    version: '0.1.9',
    notes: 'Release v0.1.9.',
    pub_date: '2026-08-24T00:00:00.000Z',
    platforms: {
      'darwin-aarch64': {
        signature: 'c2lnbmF0dXJl',
        url: ARCHIVE_URL,
      },
    },
  });
  const options = {
    version: '0.1.9',
    platform: 'darwin-aarch64',
    expectedUrl: ARCHIVE_URL,
  };

  it('accepts a valid stable feed', () => {
    expect(() => assertValidStableFeed(validFeed(), options)).not.toThrow();
  });

  it('rejects a feed version that does not match the release version', () => {
    const feed = validFeed();
    feed.version = '0.1.8';
    expect(() => assertValidStableFeed(feed, options)).toThrow(/does not match release version/);
  });

  it('rejects a feed missing the darwin-aarch64 platform', () => {
    const feed = validFeed();
    feed.platforms = { 'linux-x86_64': feed.platforms['darwin-aarch64'] } as never;
    expect(() => assertValidStableFeed(feed, options)).toThrow(
      /missing required platform "darwin-aarch64"/
    );
  });

  it('rejects a feed with no platforms object', () => {
    const feed = validFeed();
    delete (feed as Record<string, unknown>).platforms;
    expect(() => assertValidStableFeed(feed, options)).toThrow(/"platforms" object/);
  });

  it('rejects an empty signature', () => {
    const feed = validFeed();
    feed.platforms['darwin-aarch64'].signature = '  ';
    expect(() => assertValidStableFeed(feed, options)).toThrow(/empty signature/);
  });

  it('rejects a URL that does not point at the stable alias archive', () => {
    const feed = validFeed();
    feed.platforms['darwin-aarch64'].url =
      'https://github.com/block/builderbot/releases/download/staged/v0.1.9/Staged.app.tar.gz';
    expect(() => assertValidStableFeed(feed, options)).toThrow(
      /does not match the expected stable alias asset/
    );
  });

  it('rejects non-object feeds', () => {
    expect(() => assertValidStableFeed(null, options)).toThrow(/JSON object/);
  });
});

describe('assertArchiveEmbedsVersion', () => {
  let archiveDir: string;
  const archivePath = () => join(archiveDir, 'Staged.app.tar.gz');

  beforeEach(() => {
    archiveDir = mkdtempSync(join(tmpdir(), 'staged-release-checks-'));
  });

  afterEach(() => {
    rmSync(archiveDir, { recursive: true, force: true });
  });

  it('accepts an archive whose Info.plist embeds the release version', () => {
    writeAppArchive(archivePath(), { plist: infoPlist('1.2.3') });
    expect(() => assertArchiveEmbedsVersion(archivePath(), '1.2.3')).not.toThrow();
  });

  it('does not depend on the app bundle name', () => {
    writeAppArchive(archivePath(), { plist: infoPlist('1.2.3'), appName: 'Other.app' });
    expect(() => assertArchiveEmbedsVersion(archivePath(), '1.2.3')).not.toThrow();
  });

  it('rejects an archive built from a different version', () => {
    writeAppArchive(archivePath(), { plist: infoPlist('1.2.2') });
    expect(() => assertArchiveEmbedsVersion(archivePath(), '1.2.3')).toThrow(
      /embeds version "1\.2\.2" but publishing version "1\.2\.3"/
    );
  });

  it('rejects an archive without a top-level Info.plist', () => {
    writeAppArchive(archivePath(), { plist: null });
    expect(() => assertArchiveEmbedsVersion(archivePath(), '1.2.3')).toThrow(
      /Expected exactly one <Name>\.app\/Contents\/Info\.plist .* found 0/
    );
  });

  it('rejects an Info.plist without CFBundleShortVersionString', () => {
    writeAppArchive(archivePath(), { plist: 'bplist00\u0000binary' });
    expect(() => assertArchiveEmbedsVersion(archivePath(), '1.2.3')).toThrow(
      /Could not read CFBundleShortVersionString/
    );
  });

  it('rejects a file that is not a tar.gz archive', () => {
    writeFileSync(archivePath(), 'archive-bytes');
    expect(() => assertArchiveEmbedsVersion(archivePath(), '1.2.3')).toThrow();
  });
});

describe('assertValidHttpsAssetUrl', () => {
  it('accepts a plain https URL', () => {
    expect(() => assertValidHttpsAssetUrl(ARCHIVE_URL)).not.toThrow();
  });

  it('rejects http and credentialed URLs', () => {
    expect(() => assertValidHttpsAssetUrl('http://example.com/a.tar.gz')).toThrow(/must use https/);
    expect(() => assertValidHttpsAssetUrl('https://u:p@example.com/a.tar.gz')).toThrow(
      /must not embed credentials/
    );
  });
});

describe('assertUrlReachable', () => {
  it('resolves when HEAD succeeds', async () => {
    const fetchImpl = vi.fn().mockResolvedValue({ ok: true, status: 200 });
    await expect(
      assertUrlReachable(ARCHIVE_URL, { fetchImpl, attempts: 1, delayMs: 0 })
    ).resolves.toBeUndefined();
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('falls back to a ranged GET when HEAD is rejected', async () => {
    // 206 Partial Content is within Response.ok's 200–299 range.
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce({ ok: false, status: 403 })
      .mockResolvedValueOnce({ ok: true, status: 206 });
    await expect(
      assertUrlReachable(ARCHIVE_URL, { fetchImpl, attempts: 1, delayMs: 0 })
    ).resolves.toBeUndefined();
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('retries and fails when the asset never becomes reachable', async () => {
    const fetchImpl = vi.fn().mockResolvedValue({ ok: false, status: 404 });
    await expect(
      assertUrlReachable(ARCHIVE_URL, { fetchImpl, attempts: 2, delayMs: 0 })
    ).rejects.toThrow(/not reachable/);
    expect(fetchImpl).toHaveBeenCalledTimes(4);
  });

  it('treats network errors as retryable failures', async () => {
    const fetchImpl = vi.fn().mockRejectedValue(new Error('ECONNRESET'));
    await expect(
      assertUrlReachable(ARCHIVE_URL, { fetchImpl, attempts: 1, delayMs: 0 })
    ).rejects.toThrow(/ECONNRESET/);
  });
});
