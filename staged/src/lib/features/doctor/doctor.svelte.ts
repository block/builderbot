/**
 * doctor.svelte.ts — Reactive state for the system health-check modal.
 *
 * Exposes `doctorState` (report + loading flag) and an action helper
 * that calls the Tauri `run_doctor` command.
 */
import { runDoctor, type DoctorReport } from '../../commands';

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
