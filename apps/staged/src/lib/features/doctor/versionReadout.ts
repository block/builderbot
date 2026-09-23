/**
 * versionReadout.ts — what a Doctor row shows for one binary behind an agent
 * check, kept pure so it can be tested without rendering the row.
 *
 * An agent check fronts up to two binaries, each with its own readout: the
 * agent's own CLI (`main`) and its ACP bridge (`bridge`). For the bridges
 * Staged manages (Claude, Codex) both readouts describe one executable — the
 * ACP package vendors the agent — and only the ACP readout carries an update,
 * because the package is the update unit. The row shows every installed
 * version it knows, whether or not an update is available, and never turns an
 * unknown or failed registry lookup into "up to date".
 */
import type { AgentVersionInfo } from '../../api/commands';

/** Which binary behind the check a readout describes. */
export type ReadoutKind = 'main' | 'bridge';

export const READOUT_LABELS: Record<ReadoutKind, string> = {
  main: 'Main agent',
  bridge: 'ACP bridge',
};

/** Shown in place of a path for a binary Staged installed and updates itself. */
export const MANAGED_LOCATION = 'Managed by Staged';

export interface UpdateBadge {
  /** The existing badge text, e.g. `Bridge update available: 0.16.2 → 0.17.0`. */
  text: string;
  /** No command runs for this update: the badge is informational only. */
  infoOnly: boolean;
}

export interface VersionReadoutView {
  /** `Main agent` or `ACP bridge`. */
  label: string;
  /** Where the binary came from: `Managed by Staged`, else its resolved path. */
  location: string;
  managed: boolean;
  /** The installed version, when known. */
  version: string | null;
  /**
   * Why `version` may be null: `checking` while the freshness pass that probes
   * it is still running, `unknown` once it has finished without one.
   */
  versionState: 'known' | 'checking' | 'unknown';
  /**
   * The latest release of the same package is known and is not newer. False
   * whenever the lookup failed or was suppressed — unknown is not current.
   */
  upToDate: boolean;
  /** The update badge, when a newer release is known. */
  badge: UpdateBadge | null;
}

export function describeVersionReadout(args: {
  kind: ReadoutKind;
  path: string;
  info: AgentVersionInfo | null;
  /** The freshness pass is in flight, so a missing version may still arrive. */
  loading: boolean;
}): VersionReadoutView {
  const { kind, path, info, loading } = args;
  const managed = info?.bundled === true;
  const version = info?.installedVersion ?? null;
  const versionState = version !== null ? 'known' : loading ? 'checking' : 'unknown';
  return {
    label: READOUT_LABELS[kind],
    location: managed ? MANAGED_LOCATION : path,
    managed,
    version,
    versionState,
    upToDate: version !== null && info?.updateAvailable === false,
    badge: updateBadge(kind, info),
  };
}

/** The update badge a readout surfaces, worded as the row always has. */
export function updateBadge(kind: ReadoutKind, info: AgentVersionInfo | null): UpdateBadge | null {
  if (info?.updateAvailable !== true) return null;
  const subject = kind === 'bridge' ? 'Bridge update' : 'Update';
  return {
    text: `${subject} available: ${info.installedVersion ?? '?'} → ${info.latestVersion ?? '?'}`,
    infoOnly: !info.updateCommand,
  };
}
