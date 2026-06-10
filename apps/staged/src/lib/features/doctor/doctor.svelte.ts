/**
 * doctor.svelte.ts — Reactive state for the system health-check UI.
 *
 * Exposes `doctorState` (report + loading flag) and an action helper
 * that calls the Tauri `run_doctor` command.
 */
import {
  runDoctor,
  runDoctorFreshness,
  runDoctorUpdate,
  type AgentVersionInfo,
  type DoctorCheck,
  type DoctorReport,
} from '../../api/commands';

interface DoctorState {
  /** The most recent report, or null if not yet run. */
  report: DoctorReport | null;
  /** True while the initial scan is running. */
  loading: boolean;
  /** True while the async version-freshness pass is running. */
  freshnessLoading: boolean;
  /** IDs of checks with an update currently in flight (blocks double-clicks). */
  updating: string[];
}

export const doctorState: DoctorState = $state({
  report: null,
  loading: false,
  freshnessLoading: false,
  updating: [],
});

/** Version fields the freshness pass fills in; merged onto the base report. */
const FRESHNESS_FIELDS: (keyof DoctorCheck)[] = [
  'installedVersion',
  'latestVersion',
  'updateAvailable',
  'installSource',
  'selfUpdating',
  'main',
  'bridge',
];

/** A readout is actionable when it has both an update and a command to run it. */
export function isReadoutActionable(readout: AgentVersionInfo | null | undefined): boolean {
  return !!readout && readout.updateAvailable === true && !!readout.updateCommand;
}

/** True when either readout of a check has an actionable update available. */
export function hasActionableUpdate(check: DoctorCheck): boolean {
  return isReadoutActionable(check.main) || isReadoutActionable(check.bridge);
}

const STATUS_ICONS: Record<DoctorCheck['status'], string> = {
  pass: '✓',
  warn: '⚠',
  fail: '✗',
};

/** Format the full doctor report as a plain-text debug dump for support. */
export function formatDebugReport(report: DoctorReport): string {
  const lines: string[] = [
    'Staged Doctor Report',
    `Date: ${new Date().toISOString()}`,
    '='.repeat(60),
  ];

  for (const check of report.checks) {
    const icon = STATUS_ICONS[check.status];
    lines.push('');
    lines.push(`${icon} [${check.status.toUpperCase()}] ${check.label} (${check.id})`);
    lines.push(`  Message: ${check.message}`);
    if (check.path) lines.push(`  Path: ${check.path}`);
    if (check.bridgePath) lines.push(`  Bridge path: ${check.bridgePath}`);
    if (check.fixUrl) lines.push(`  Fix URL: ${check.fixUrl}`);
    if (check.fixCommand) lines.push(`  Fix command: ${check.fixCommand}`);
    if (check.rawOutput) {
      lines.push('  --- raw output ---');
      for (const line of check.rawOutput.split('\n')) {
        lines.push(`  ${line}`);
      }
      lines.push('  --- end raw output ---');
    }
  }

  lines.push('');
  lines.push('='.repeat(60));
  return lines.join('\n');
}

/**
 * Run all checks and update the reactive state.
 *
 * Returns a promise that resolves once the report is ready.
 * Does NOT call refreshProviders — callers that care about
 * updating the agent selector should do so themselves after
 * awaiting this function.
 */
export async function runChecks(): Promise<void> {
  doctorState.loading = true;
  try {
    doctorState.report = await runDoctor();
  } catch (e) {
    console.error('[Doctor] Failed to run checks:', e);
  } finally {
    doctorState.loading = false;
  }

  // Second pass: probe registries for version freshness and merge the version
  // fields onto the already-painted report. Don't await — let the base report
  // render immediately while this fills in update badges in the background.
  void refreshFreshness();
}

/**
 * Monotonic token identifying the most recently started freshness pass.
 *
 * Passes can overlap (a re-run or an update fires while one is in flight) and
 * resolve out of order. Each pass captures the value here at the start; only
 * the latest pass is allowed to merge its result or own `freshnessLoading`.
 */
let freshnessGeneration = 0;

/**
 * Run the freshness pass and merge its version fields into the current report.
 *
 * Patches per-check rather than replacing the whole report, so the visible rows
 * never blank out (the freshness report can lag or differ in transient ways).
 */
export async function refreshFreshness(): Promise<void> {
  if (!doctorState.report) return;
  const generation = ++freshnessGeneration;
  doctorState.freshnessLoading = true;
  try {
    const fresh = await runDoctorFreshness();
    // A newer pass started while this one was in flight: discard this result
    // entirely. Merging would patch stale version data, and the newer pass
    // already owns `freshnessLoading`.
    if (generation !== freshnessGeneration) return;
    mergeFreshness(fresh);
  } catch (e) {
    console.error('[Doctor] Failed to run freshness pass:', e);
  } finally {
    // Only the latest pass clears the flag, so a superseded pass resolving late
    // can't switch the spinner off while a newer pass is still running.
    if (generation === freshnessGeneration) doctorState.freshnessLoading = false;
  }
}

/** Merge version fields from a freshness report into the cached base report. */
function mergeFreshness(fresh: DoctorReport): void {
  const report = doctorState.report;
  if (!report) return;
  const byId = new Map(fresh.checks.map((c) => [c.id, c]));
  for (const check of report.checks) {
    const updated = byId.get(check.id);
    if (!updated) continue;
    for (const field of FRESHNESS_FIELDS) {
      // Assign through a loosely-typed view: every field is independently
      // optional and we're copying like-for-like from the same shape.
      (check as unknown as Record<string, unknown>)[field] = updated[field];
    }
  }
}

/**
 * Update a single check's actionable readouts (main CLI + ACP bridge).
 *
 * Runs the readouts' update commands **sequentially** — never race two global
 * npm/brew installs, they can clobber each other. After the updates land, the
 * caller is expected to refresh freshness so the badges clear. Guards against
 * double-clicks via the per-check in-flight set.
 *
 * Returns true if at least one update ran successfully.
 */
export async function updateCheck(check: DoctorCheck): Promise<boolean> {
  if (doctorState.updating.includes(check.id)) return false;
  doctorState.updating = [...doctorState.updating, check.id];
  let ranAny = false;
  try {
    for (const readout of [check.main, check.bridge]) {
      if (!isReadoutActionable(readout) || !readout?.updateFixType || !readout.updateCommand) {
        continue;
      }
      await runDoctorUpdate(check.id, readout.updateFixType, readout.updateCommand);
      ranAny = true;
    }
  } finally {
    doctorState.updating = doctorState.updating.filter((id) => id !== check.id);
  }
  return ranAny;
}

/**
 * Update every check that has an actionable readout, one check at a time.
 *
 * Serialized across checks (and within each check) to avoid concurrent global
 * installs. The caller is responsible for the single full re-run afterwards
 * (a freshness-only pass would leave updated tools showing stale status/message).
 */
export async function updateAll(): Promise<void> {
  const checks = doctorState.report?.checks.filter(hasActionableUpdate) ?? [];
  for (const check of checks) {
    try {
      await updateCheck(check);
    } catch (e) {
      console.error(`[Doctor] Failed to update ${check.id}:`, e);
    }
  }
}
