/**
 * doctor.svelte.ts — Reactive state for the system health-check UI.
 *
 * Exposes `doctorState` (report + loading flag) and an action helper
 * that calls the Tauri `run_doctor` command.
 */
import { runDoctor, type DoctorReport } from '../../api/commands';
import { refreshProviders } from '../agents/agent.svelte';

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
    // Re-discover providers so newly-installed agents are immediately
    // available in the agent selector without requiring an app reload.
    await refreshProviders();
  } catch (e) {
    console.error('[Doctor] Failed to run checks:', e);
  } finally {
    doctorState.loading = false;
  }
}
