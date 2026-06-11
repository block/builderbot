/**
 * switchTracer — lightweight instrumentation for diagnosing project-switch latency.
 *
 * A "switch" is the window between navigating from one project to another and
 * the UI settling on the next one. We trace the synchronous milestones
 * (selectProject, ProjectSection mount/destroy, the App selectedProjectId
 * effect), count component mounts/unmounts, and watch requestAnimationFrame for
 * main-thread stalls — so we can tell genuine rendering cost apart from
 * event-loop starvation (e.g. a PR-poll storm blocking the renderer).
 *
 * Everything here is console-only. Lifecycle lines are prefixed `[switch #N ...]`;
 * one-off slow-path warnings emitted from other modules use the bare `[switch]`
 * prefix (see persistentStore.ts / prPollingService.ts).
 *
 * Known limitation: frame gaps larger than THROTTLE_MS are assumed to be
 * background/window throttling and are NOT counted as stalls — so a multi-second
 * full freeze shows up as `stalls: count=0` with a large `settled`. Read the gap
 * between `firstFrame` and the marks to spot those.
 */

const QUIET_MS = 300; // settle once this long elapses with no recorded activity
const MAX_MS = 3000; // hard cap — settle even if activity never goes quiet
const STALL_MS = 50; // a frame gap >= this (and <= THROTTLE_MS) counts as a stall
const THROTTLE_MS = 700; // gaps larger than this are treated as background throttling

interface Mark {
  label: string;
  at: number; // ms since switch begin
}

interface Stall {
  at: number; // ms since begin where the stall started (last good frame)
  dur: number; // gap length in ms
}

interface ActiveSwitch {
  id: number;
  from: string;
  to: string;
  t0: number;
  marks: Mark[];
  mounts: Record<string, number>;
  unmounts: Record<string, number>;
  sync: Record<string, number>;
  stalls: Stall[];
  firstFrame: number | null;
  lastActivity: number;
  lastFrame: number;
  rafId: number | null;
  timeoutId: ReturnType<typeof setTimeout> | null;
  done: boolean;
}

let active: ActiveSwitch | null = null;
let switchSeq = 0;

function now(): number {
  return performance.now();
}

function offset(sw: ActiveSwitch): number {
  return Math.round(now() - sw.t0);
}

/**
 * Begin tracing a project→project switch. No-ops for home↔project navigation
 * (null `from`) and same-project re-selection. If a prior switch is still
 * settling it is finalized first.
 */
export function beginSwitch(from: string | null, to: string): void {
  if (!from || from === to) return;
  if (active && !active.done) finalize(active);

  switchSeq += 1;
  const t0 = now();
  const sw: ActiveSwitch = {
    id: switchSeq,
    from,
    to,
    t0,
    marks: [],
    mounts: {},
    unmounts: {},
    sync: {},
    stalls: [],
    firstFrame: null,
    lastActivity: t0,
    lastFrame: t0,
    rafId: null,
    timeoutId: null,
    done: false,
  };
  active = sw;
  console.info(`[switch #${sw.id} +0ms] begin: ${from} → ${to}`);
  scheduleFrame(sw);
  // Safety net in case rAF never fires (e.g. window fully backgrounded).
  sw.timeoutId = setTimeout(() => finalize(sw), MAX_MS + 250);
}

/** Record a named milestone on the active switch's timeline. */
export function mark(label: string, detail?: string): void {
  const sw = active;
  if (!sw || sw.done) return;
  const at = offset(sw);
  sw.marks.push({ label, at });
  sw.lastActivity = now();
  console.info(`[switch #${sw.id} +${at}ms] ${label}${detail ? ` (${detail})` : ''}`);
}

/** Count a component mount against the active switch. */
export function countMount(component: string): void {
  const sw = active;
  if (!sw || sw.done) return;
  sw.mounts[component] = (sw.mounts[component] ?? 0) + 1;
  sw.lastActivity = now();
}

/** Count a component unmount against the active switch. */
export function countUnmount(component: string): void {
  const sw = active;
  if (!sw || sw.done) return;
  sw.unmounts[component] = (sw.unmounts[component] ?? 0) + 1;
  sw.lastActivity = now();
}

/** Accumulate the duration of a synchronous block (e.g. cache hydration). */
export function recordSync(label: string, ms: number): void {
  const sw = active;
  if (!sw || sw.done) return;
  sw.sync[label] = (sw.sync[label] ?? 0) + ms;
}

function scheduleFrame(sw: ActiveSwitch): void {
  sw.rafId = requestAnimationFrame(() => onFrame(sw));
}

function onFrame(sw: ActiveSwitch): void {
  if (sw.done) return;
  const t = now();
  const sinceStart = t - sw.t0;
  const dt = t - sw.lastFrame;

  if (sw.firstFrame === null) {
    sw.firstFrame = Math.round(sinceStart);
  }

  // A frame gap in [STALL_MS, THROTTLE_MS] is a main-thread stall. Larger gaps
  // are treated as background throttling and intentionally ignored.
  if (dt >= STALL_MS && dt <= THROTTLE_MS) {
    sw.stalls.push({ at: Math.round(sw.lastFrame - sw.t0), dur: Math.round(dt) });
  }
  sw.lastFrame = t;

  const sinceActivity = t - sw.lastActivity;
  if (sinceActivity >= QUIET_MS || sinceStart > MAX_MS) {
    finalize(sw);
    return;
  }
  scheduleFrame(sw);
}

function fmtCounts(rec: Record<string, number>): string {
  const entries = Object.entries(rec).map(([k, v]) => `${k}=${v}`);
  return entries.length ? entries.join(' ') : 'none';
}

function fmtSync(rec: Record<string, number>): string {
  const entries = Object.entries(rec).map(([k, v]) => `${k}=${Math.round(v)}ms`);
  return entries.length ? entries.join(' ') : 'none';
}

function finalize(sw: ActiveSwitch): void {
  if (sw.done) return;
  sw.done = true;
  if (sw.rafId !== null) cancelAnimationFrame(sw.rafId);
  if (sw.timeoutId !== null) clearTimeout(sw.timeoutId);
  if (active === sw) active = null;

  const settled = offset(sw);
  const firstFrame = sw.firstFrame ?? settled;
  const stallCount = sw.stalls.length;
  const stallMax = sw.stalls.reduce((m, s) => Math.max(m, s.dur), 0);
  const stallTotal = sw.stalls.reduce((s, x) => s + x.dur, 0);
  const marksStr = sw.marks.map((m) => `${m.label}@+${m.at}ms`).join(', ');

  let line =
    `[switch #${sw.id} done] ${sw.from} → ${sw.to}` +
    ` | settled +${settled}ms` +
    ` | firstFrame +${firstFrame}ms` +
    ` | stalls: count=${stallCount} max=${stallMax}ms total=${stallTotal}ms` +
    ` | mounts: ${fmtCounts(sw.mounts)}` +
    ` | unmounts: ${fmtCounts(sw.unmounts)}` +
    ` | sync: ${fmtSync(sw.sync)}` +
    ` | marks: [${marksStr}]`;
  if (stallCount > 0) {
    const detail = sw.stalls.map((s) => `+${s.at}ms:${s.dur}ms`).join(', ');
    line += ` | stallDetail: [${detail}]`;
  }
  console.info(line);
}
