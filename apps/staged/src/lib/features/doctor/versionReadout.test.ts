import { describe, expect, it } from 'vitest';
import type { AgentVersionInfo } from '../../api/commands';
import { MANAGED_LOCATION, describeVersionReadout, updateBadge } from './versionReadout';

const SHIM = '/Users/me/.staged/packages/bin/claude-agent-acp';

function readout(overrides: Partial<AgentVersionInfo> = {}): AgentVersionInfo {
  return {
    installSource: 'npm',
    installedVersion: null,
    latestVersion: null,
    updateAvailable: null,
    selfUpdating: null,
    updateCommand: null,
    updateFixType: null,
    bundled: null,
    ...overrides,
  };
}

/** The two readouts a managed Claude row carries: one executable, two versions. */
function managedMain(overrides: Partial<AgentVersionInfo> = {}): AgentVersionInfo {
  return readout({
    installSource: 'bundled',
    bundled: true,
    installedVersion: '2.1.205',
    selfUpdating: true,
    ...overrides,
  });
}

function managedBridge(overrides: Partial<AgentVersionInfo> = {}): AgentVersionInfo {
  return readout({
    installSource: 'bundled',
    bundled: true,
    installedVersion: '0.16.2',
    selfUpdating: false,
    ...overrides,
  });
}

describe('describeVersionReadout', () => {
  it('shows the main agent and ACP bridge versions of a managed install separately', () => {
    const main = describeVersionReadout({
      kind: 'main',
      path: SHIM,
      info: managedMain(),
      loading: false,
    });
    const bridge = describeVersionReadout({
      kind: 'bridge',
      path: SHIM,
      info: managedBridge(),
      loading: false,
    });

    expect(main.label).toBe('Main agent');
    expect(bridge.label).toBe('ACP bridge');
    // The same executable, described twice — and never by its shim path.
    expect(main.location).toBe(MANAGED_LOCATION);
    expect(bridge.location).toBe(MANAGED_LOCATION);
    expect(main.managed).toBe(true);
    expect(bridge.managed).toBe(true);
    // Each version is its own, shown even though neither has an update.
    expect(main.version).toBe('2.1.205');
    expect(bridge.version).toBe('0.16.2');
    expect(main.versionState).toBe('known');
    expect(bridge.versionState).toBe('known');
    expect(main.badge).toBeNull();
    expect(bridge.badge).toBeNull();
  });

  it('badges the ACP readout when the registry has a newer release of the same package', () => {
    const view = describeVersionReadout({
      kind: 'bridge',
      path: SHIM,
      info: managedBridge({
        latestVersion: '0.17.0',
        updateAvailable: true,
        updateCommand: "update Staged's managed @agentclientprotocol/claude-agent-acp to 0.17.0",
        updateFixType: 'updateBridge',
      }),
      loading: false,
    });

    expect(view.version).toBe('0.16.2');
    expect(view.upToDate).toBe(false);
    expect(view.badge).toEqual({
      text: 'Bridge update available: 0.16.2 → 0.17.0',
      infoOnly: false,
    });
  });

  it('marks an equal release as up to date with no badge', () => {
    const view = describeVersionReadout({
      kind: 'bridge',
      path: SHIM,
      info: managedBridge({ latestVersion: '0.16.2', updateAvailable: false }),
      loading: false,
    });

    expect(view.version).toBe('0.16.2');
    expect(view.upToDate).toBe(true);
    expect(view.badge).toBeNull();
  });

  it('keeps an unknown or unavailable registry answer unknown, not up to date', () => {
    // Lookup failed (offline, mirror down, timeout): installed still shows.
    const unknown = describeVersionReadout({
      kind: 'bridge',
      path: SHIM,
      info: managedBridge({ latestVersion: null, updateAvailable: null }),
      loading: false,
    });
    expect(unknown.version).toBe('0.16.2');
    expect(unknown.upToDate).toBe(false);
    expect(unknown.badge).toBeNull();

    // Registry answered but the installed version could not be read.
    const noInstalled = describeVersionReadout({
      kind: 'bridge',
      path: SHIM,
      info: managedBridge({ installedVersion: null, latestVersion: '0.17.0' }),
      loading: false,
    });
    expect(noInstalled.version).toBeNull();
    expect(noInstalled.versionState).toBe('unknown');
    expect(noInstalled.upToDate).toBe(false);
    expect(noInstalled.badge).toBeNull();

    // The vendored agent's readout has its update suppressed by the shared
    // bundled policy (null, not false): it is never called up to date, since
    // the latest ACP package need not carry the latest standalone CLI.
    const main = describeVersionReadout({
      kind: 'main',
      path: SHIM,
      info: managedMain({ latestVersion: null, updateAvailable: null }),
      loading: false,
    });
    expect(main.version).toBe('2.1.205');
    expect(main.upToDate).toBe(false);
  });

  it('reads "checking" only while the freshness pass can still deliver a version', () => {
    const info = managedMain({ installedVersion: null });
    expect(
      describeVersionReadout({ kind: 'main', path: SHIM, info, loading: true }).versionState
    ).toBe('checking');
    expect(
      describeVersionReadout({ kind: 'main', path: SHIM, info, loading: false }).versionState
    ).toBe('unknown');
    // A version already known is not "checking" even mid-pass.
    expect(
      describeVersionReadout({ kind: 'main', path: SHIM, info: managedMain(), loading: true })
        .versionState
    ).toBe('known');
  });

  it('shows the path and the source-aware badge for an unmanaged install', () => {
    const main = describeVersionReadout({
      kind: 'main',
      path: '/opt/homebrew/bin/amp',
      info: readout({ installSource: 'brew', installedVersion: '0.0.1758' }),
      loading: false,
    });
    expect(main.location).toBe('/opt/homebrew/bin/amp');
    expect(main.managed).toBe(false);
    expect(main.version).toBe('0.0.1758');

    const bridge = describeVersionReadout({
      kind: 'bridge',
      path: '/Users/me/.npm-global/bin/amp-acp',
      info: readout({
        installedVersion: '0.4.2',
        latestVersion: '0.5.0',
        updateAvailable: true,
        updateCommand: 'npm install -g amp-acp@latest',
        updateFixType: 'updateBridge',
      }),
      loading: false,
    });
    expect(bridge.location).toBe('/Users/me/.npm-global/bin/amp-acp');
    expect(bridge.badge).toEqual({
      text: 'Bridge update available: 0.4.2 → 0.5.0',
      infoOnly: false,
    });

    // A binary with no readout at all (resolved, nothing detected).
    const bare = describeVersionReadout({
      kind: 'main',
      path: '/usr/local/bin/goose',
      info: null,
      loading: false,
    });
    expect(bare.location).toBe('/usr/local/bin/goose');
    expect(bare.versionState).toBe('unknown');
    expect(bare.badge).toBeNull();
  });
});

describe('updateBadge', () => {
  it('is informational when a newer version is known but nothing can run it', () => {
    expect(
      updateBadge(
        'main',
        readout({ installedVersion: '1.0.0', latestVersion: '1.1.0', updateAvailable: true })
      )
    ).toEqual({ text: 'Update available: 1.0.0 → 1.1.0', infoOnly: true });
  });

  it('fills unknown versions with a placeholder rather than dropping the badge', () => {
    expect(
      updateBadge('bridge', readout({ latestVersion: '1.1.0', updateAvailable: true }))
    ).toEqual({ text: 'Bridge update available: ? → 1.1.0', infoOnly: true });
  });

  it('is absent unless an update is positively available', () => {
    expect(updateBadge('main', null)).toBeNull();
    expect(updateBadge('main', readout({ updateAvailable: false }))).toBeNull();
    expect(updateBadge('main', readout({ updateAvailable: null }))).toBeNull();
  });
});
