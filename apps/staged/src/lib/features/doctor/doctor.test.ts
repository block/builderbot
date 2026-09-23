import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentVersionInfo, DoctorCheck, DoctorReport } from '../../api/commands';

const SHIM = '/Users/me/.staged/packages/bin/claude-agent-acp';
const MANAGED_UPDATE =
  "update Staged's managed @agentclientprotocol/claude-agent-acp to 0.17.0 in ~/.staged/packages";

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

function check(overrides: Partial<DoctorCheck> & Pick<DoctorCheck, 'id'>): DoctorCheck {
  return {
    label: overrides.id,
    status: 'pass',
    message: 'Installed',
    fixUrl: null,
    fixCommand: null,
    fixType: null,
    path: null,
    bridgePath: null,
    rawOutput: null,
    authStatus: null,
    loginCommand: null,
    installedVersion: null,
    latestVersion: null,
    updateAvailable: null,
    installSource: null,
    selfUpdating: null,
    main: null,
    bridge: null,
    ...overrides,
  };
}

/**
 * A managed Claude row as the backend reports it: the vendored agent under
 * `main` (bundled, update suppressed by the shared policy) and the ACP package
 * under `bridge` on the same executable. `latest` shapes the ACP readout: a
 * newer release carries the one managed update, an equal one none, and null
 * (registry unknown) leaves the update unknown.
 */
function managedClaude(installed: string, latest: string | null): DoctorCheck {
  const updateAvailable = latest === null ? null : latest !== installed;
  return check({
    id: 'ai-agent-claude',
    label: 'Claude Code',
    path: SHIM,
    bridgePath: SHIM,
    installSource: 'bundled',
    main: readout({
      installSource: 'bundled',
      bundled: true,
      installedVersion: '2.1.205',
      selfUpdating: true,
    }),
    bridge: readout({
      installSource: 'bundled',
      bundled: true,
      installedVersion: installed,
      latestVersion: latest,
      updateAvailable,
      selfUpdating: false,
      updateCommand: updateAvailable ? MANAGED_UPDATE : null,
      updateFixType: updateAvailable ? 'updateBridge' : null,
    }),
  });
}

/** An unmanaged agent whose two binaries are separate installs with their own updates. */
function amp(): DoctorCheck {
  return check({
    id: 'ai-agent-amp',
    label: 'Amp',
    path: '/opt/homebrew/bin/amp',
    bridgePath: '/Users/me/.npm-global/bin/amp-acp',
    main: readout({
      installSource: 'brew',
      installedVersion: '0.0.1758',
      latestVersion: '0.0.1760',
      updateAvailable: true,
      updateCommand: 'brew upgrade ampcode',
      updateFixType: 'updateMain',
    }),
    bridge: readout({
      installedVersion: '0.4.2',
      latestVersion: '0.5.0',
      updateAvailable: true,
      updateCommand: 'npm install -g amp-acp@latest',
      updateFixType: 'updateBridge',
    }),
  });
}

describe('doctor store', () => {
  let runDoctor: ReturnType<typeof vi.fn>;
  let runDoctorFreshness: ReturnType<typeof vi.fn>;
  let runDoctorUpdate: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    // The store is a .svelte.ts module compiled without the Svelte plugin
    // here, so the rune call resolves to this pass-through global.
    vi.stubGlobal('$state', (initial: unknown) => initial);
    runDoctor = vi.fn();
    runDoctorFreshness = vi.fn();
    runDoctorUpdate = vi.fn().mockResolvedValue(undefined);
    vi.doMock('../../api/commands', () => ({ runDoctor, runDoctorFreshness, runDoctorUpdate }));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.doUnmock('../../api/commands');
  });

  function load() {
    return import('./doctor.svelte');
  }

  /** Let the un-awaited freshness pass behind `runChecks` merge its result. */
  function flush(): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, 0));
  }

  it('runs exactly one managed install for a row whose two readouts share one package', async () => {
    const { updateCheck } = await load();

    const ran = await updateCheck(managedClaude('0.16.2', '0.17.0'));

    expect(ran).toBe(true);
    // The vendored agent is updated *by* the ACP package install: a second
    // action on the main readout would reinstall it twice.
    expect(runDoctorUpdate).toHaveBeenCalledTimes(1);
    expect(runDoctorUpdate).toHaveBeenCalledWith('ai-agent-claude', 'updateBridge', MANAGED_UPDATE);
  });

  it('runs each independently installed readout of an unmanaged row, main first', async () => {
    const { updateCheck } = await load();

    await updateCheck(amp());

    expect(runDoctorUpdate.mock.calls).toEqual([
      ['ai-agent-amp', 'updateMain', 'brew upgrade ampcode'],
      ['ai-agent-amp', 'updateBridge', 'npm install -g amp-acp@latest'],
    ]);
  });

  it('runs nothing when the registry answer is unknown or the install is current', async () => {
    const { updateCheck, hasActionableUpdate, isReadoutActionable } = await load();

    const unknown = managedClaude('0.16.2', null);
    const current = managedClaude('0.17.0', '0.17.0');
    expect(hasActionableUpdate(unknown)).toBe(false);
    expect(hasActionableUpdate(current)).toBe(false);
    // The suppressed vendored-agent readout is never actionable on its own.
    expect(isReadoutActionable(unknown.main)).toBe(false);
    // A newer version with no way to install it is a badge, not a button.
    expect(
      isReadoutActionable(
        readout({ installedVersion: '1.0.0', latestVersion: '1.1.0', updateAvailable: true })
      )
    ).toBe(false);

    expect(await updateCheck(unknown)).toBe(false);
    expect(await updateCheck(current)).toBe(false);
    expect(runDoctorUpdate).not.toHaveBeenCalled();
  });

  it('refuses a second update for a check whose update is still running', async () => {
    const { doctorState, updateCheck } = await load();
    let finish: () => void = () => {};
    runDoctorUpdate.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        finish = resolve;
      })
    );

    const first = updateCheck(managedClaude('0.16.2', '0.17.0'));
    expect(doctorState.updating).toEqual(['ai-agent-claude']);
    expect(await updateCheck(managedClaude('0.16.2', '0.17.0'))).toBe(false);
    expect(runDoctorUpdate).toHaveBeenCalledTimes(1);

    finish();
    expect(await first).toBe(true);
    expect(doctorState.updating).toEqual([]);
  });

  it('surfaces a failed update and leaves the in-flight set clean', async () => {
    const { doctorState, updateCheck } = await load();
    runDoctorUpdate.mockRejectedValueOnce(new Error('npm install failed: exited with 1'));

    await expect(updateCheck(managedClaude('0.16.2', '0.17.0'))).rejects.toThrow(
      'npm install failed'
    );
    expect(doctorState.updating).toEqual([]);
  });

  it('shows the new versions and clears the badge once a successful update is re-checked', async () => {
    const { doctorState, runChecks, updateCheck, hasActionableUpdate } = await load();
    const report = (c: DoctorCheck): DoctorReport => ({ checks: [c] });
    // The base report already carries both readouts and both paths; the
    // freshness report adds the registry's answer.
    runDoctor.mockResolvedValueOnce(report(managedClaude('0.16.2', null)));
    runDoctorFreshness.mockResolvedValueOnce(report(managedClaude('0.16.2', '0.17.0')));

    await runChecks();
    await flush();

    let row = doctorState.report!.checks[0];
    expect(row.bridgePath).toBe(SHIM);
    expect(row.main?.installedVersion).toBe('2.1.205');
    expect(row.bridge?.installedVersion).toBe('0.16.2');
    expect(row.bridge?.latestVersion).toBe('0.17.0');
    expect(hasActionableUpdate(row)).toBe(true);
    expect(doctorState.freshnessLoading).toBe(false);

    // The update lands, and the row's `onFixed` re-runs the checks against
    // the install now on disk.
    await updateCheck(row);
    expect(runDoctorUpdate).toHaveBeenCalledTimes(1);
    runDoctor.mockResolvedValueOnce(report(managedClaude('0.17.0', null)));
    runDoctorFreshness.mockResolvedValueOnce(report(managedClaude('0.17.0', '0.17.0')));

    await runChecks();
    await flush();

    row = doctorState.report!.checks[0];
    expect(row.bridge?.installedVersion).toBe('0.17.0');
    expect(row.bridge?.updateAvailable).toBe(false);
    expect(row.bridge?.updateCommand).toBeNull();
    expect(hasActionableUpdate(row)).toBe(false);
    // The vendored agent's readout is still shown, still without an update.
    expect(row.main?.installedVersion).toBe('2.1.205');
    expect(row.main?.updateAvailable).toBeNull();
  });

  it('merges freshness readouts onto the base report without dropping its paths', async () => {
    const { doctorState, runChecks } = await load();
    const base = managedClaude('0.16.2', null);
    const fresh = managedClaude('0.16.2', '0.17.0');
    runDoctor.mockResolvedValueOnce({ checks: [base, amp()] });
    runDoctorFreshness.mockResolvedValueOnce({ checks: [fresh] });

    await runChecks();
    await flush();

    const [claude, ampRow] = doctorState.report!.checks;
    expect(claude.path).toBe(SHIM);
    expect(claude.bridgePath).toBe(SHIM);
    expect(claude.bridge).toEqual(fresh.bridge);
    // A check the freshness report did not cover keeps its base readouts.
    expect(ampRow.bridge?.installedVersion).toBe('0.4.2');
  });

  it('lists each readout version in the debug report', async () => {
    const { formatDebugReport } = await load();

    const text = formatDebugReport({ checks: [managedClaude('0.16.2', '0.17.0'), amp()] });

    expect(text).toContain('  Main version: 2.1.205\n');
    expect(text).toContain('  Bridge version: 0.16.2 (latest 0.17.0)\n');
    expect(text).toContain('  Main version: 0.0.1758 (latest 0.0.1760)\n');
  });
});
