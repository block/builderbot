/**
 * Whether a Tab press is in flight, and which way. App.svelte's window
 * handlers write it on keydown / keyup / blur; the focus guards on covered
 * views (App's parked workspace, WorkspaceLayer) read it to tell a Tab that
 * landed in a hidden view — forward it to the next control — from focus that
 * got there by click or `.focus()`, which goes back where it came from.
 */
export const tabDirection = {
  inFlight: null as 'forward' | 'backward' | null,
};
