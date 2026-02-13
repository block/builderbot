/**
 * doctor.svelte.ts — Reactive state for the system health-check modal.
 *
 * Exposes `doctorState` (report + loading flags) and action helpers
 * that call the Tauri `run_doctor` / `run_doctor_fix` commands.
 */
import { runDoctor, runDoctorFix, type DoctorCheck, type DoctorReport } from '../../commands';

interface DoctorState {
  /** The most recent report, or null if not yet run. */
  report: DoctorReport | null;
  /** True while the initial scan is running. */
  loading: boolean;
  /** Set of check IDs currently being fixed. */
  fixing: Set<string>;
}

export const doctorState: DoctorState = $state({
  report: null,
  loading: false,
  fixing: new Set(),
});

/** Run all checks and update the reactive state. */
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

/** Execute the fix for a single check, then update its entry in the report. */
export async function fixCheck(checkId: string): Promise<void> {
  doctorState.fixing.add(checkId);
  // Force reactivity — Svelte 5 tracks Set mutations via reassignment.
  doctorState.fixing = new Set(doctorState.fixing);

  try {
    const updated: DoctorCheck = await runDoctorFix(checkId);

    // Replace the check in the report with the updated version.
    if (doctorState.report) {
      doctorState.report = {
        ...doctorState.report,
        checks: doctorState.report.checks.map((c) => (c.id === checkId ? updated : c)),
      };
    }
  } catch (e) {
    console.error(`[Doctor] Fix failed for ${checkId}:`, e);
  } finally {
    doctorState.fixing.delete(checkId);
    doctorState.fixing = new Set(doctorState.fixing);
  }
}
