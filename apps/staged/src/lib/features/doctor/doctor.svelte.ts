/**
 * doctor.svelte.ts — Reactive state for the system health-check UI.
 *
 * Exposes `doctorState` (report + loading flag) and an action helper
 * that calls the Tauri `run_doctor` command.
 */
import { runDoctor, type DoctorCheck, type DoctorReport } from '../../api/commands';

interface DoctorState {
  /** The most recent report, or null if not yet run. */
  report: DoctorReport | null;
  /** True while the initial scan is running. */
  loading: boolean;
}

export const doctorState: DoctorState = $state({
  report: null,
  loading: false,
});

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
}
