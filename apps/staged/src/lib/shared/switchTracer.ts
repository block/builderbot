/**
 * switchTracer — lightweight instrumentation for diagnosing project-switch latency.
 *
 * A "switch" is the window between navigating from one project to another and
 * the UI settling on the next one. We trace the synchronous milestones that
 * subdivide the reactive cascade (selectProject, ProjectHome derive,
 * ProjectSection construct/mount/destroy, the App selectedProjectId effect),
 * count component mounts/unmounts, and watch requestAnimationFrame for
 * main-thread stalls — so we can tell genuine rendering cost apart from
 * event-loop starvation (e.g. a PR-poll storm blocking the renderer), and so a
 * freeze names the offending stage (derive vs. old-subtree teardown vs.
 * new-subtree build) rather than landing in one opaque span.
 *
 * Everything here is console-only. Lifecycle lines are prefixed `[switch #N ...]`;
 * one-off slow-path warnings emitted from other modules use the bare `[switch]`
 * prefix (see persistentStore.ts / prPollingService.ts).
 *
 * Frame gaps are how we detect a blocked main thread: requestAnimationFrame can
 * only fire when the thread is free, so the gap between two callbacks is the
 * length of whatever synchronous work ran in between. We classify a gap as a
 * real stall vs. background throttling by *visibility*, not by size — a large
 * gap while the window stayed visible is a genuine foreground freeze (the case
 * we are hunting), whereas the browser pauses rAF for hidden/backgrounded
 * windows. `maxGap` is always reported (never suppressed) so a multi-second
 * freeze can't hide as `stalls: count=0`, and any gap that crossed a
 * `document.hidden` period is excluded from the stall count.
 *
 * A *continuous* freeze — one synchronous block that spans the whole switch — is
 * the worst case: not a single rAF callback fires, so `onFrame` never runs and
 * the freeze would otherwise vanish as `maxGap 0 / stalls 0` (the fingerprint is
 * `firstFrame` never leaving null). `finalize` closes that blind spot: it
 * measures the trailing gap from the last frame it saw to settle time and
 * synthesizes the implied stall, so a freeze can't slip through the post-freeze
 * race where the safety-net timer fires `finalize` before the long-overdue
 * `onFrame`. The `done` line then reports two extra fields that name *where* a
 * freeze sits: `firstFrame: none` when no frame ever fired, and `freezeBracket`
 * — the largest gap between two consecutive synchronous milestones (marks),
 * e.g. `selectProject sync complete → ProjectSection destroy 6220ms` — which
 * points at the un-instrumented synchronous span to profile next.
 */

const QUIET_MS = 300; // settle once this long elapses with no recorded activity
const MAX_MS = 3000; // hard cap — settle even if activity never goes quiet
const STALL_MS = 50; // a foreground frame gap >= this counts as a stall (no upper bound)
const FREEZE_MS = 700; // a foreground stall >= this is logged immediately as a freeze

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
  maxGap: number; // largest frame gap seen, ms — reported even when it's throttling
  firstFrame: number | null;
  lastActivity: number;
  lastFrame: number;
  rafId: number | null;
  timeoutId: ReturnType<typeof setTimeout> | null;
  done: boolean;
}

let active: ActiveSwitch | null = null;
let switchSeq = 0;

// Set by a visibilitychange listener whenever the window goes hidden; consulted
// (and reset) on each frame so a gap that spanned a hidden period is classified
// as background throttling rather than a genuine foreground stall.
let sawHiddenSinceFrame = false;
let visibilityListenerAttached = false;
function ensureVisibilityListener(): void {
  if (visibilityListenerAttached || typeof document === 'undefined') return;
  visibilityListenerAttached = true;
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) sawHiddenSinceFrame = true;
  });
}

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
    maxGap: 0,
    firstFrame: null,
    lastActivity: t0,
    lastFrame: t0,
    rafId: null,
    timeoutId: null,
    done: false,
  };
  active = sw;
  sawHiddenSinceFrame = false;
  ensureVisibilityListener();
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
  const hiddenDuringGap =
    sawHiddenSinceFrame || (typeof document !== 'undefined' && document.hidden);
  sawHiddenSinceFrame = false;

  if (sw.firstFrame === null) {
    sw.firstFrame = Math.round(sinceStart);
  }

  if (dt > sw.maxGap) sw.maxGap = Math.round(dt);

  // A frame gap means the main thread was busy for its whole duration. We count
  // it as a real stall only when the window stayed visible across the gap — a
  // gap that crossed a hidden period is background throttling (the browser
  // pauses rAF for hidden windows), not a freeze. There is intentionally no
  // upper bound: a multi-second foreground gap is exactly the freeze we hunt.
  if (dt >= STALL_MS && !hiddenDuringGap) {
    const at = Math.round(sw.lastFrame - sw.t0);
    sw.stalls.push({ at, dur: Math.round(dt) });
    if (dt >= FREEZE_MS) {
      console.info(
        `[switch #${sw.id} +${at}ms] main-thread frozen for ${Math.round(dt)}ms (foreground)`
      );
    }
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

/**
 * Find the largest gap between two consecutive synchronous milestones on the
 * timeline `begin → ...marks → settled`. Marks are recorded even during a total
 * freeze (they fire from the reactive flush itself), so this localizes the
 * freeze to the span between two named milestones — the closest the logs can get
 * to naming the un-instrumented synchronous block without a profiler.
 */
function largestMarkGap(
  sw: ActiveSwitch,
  settled: number
): { from: string; to: string; gap: number } {
  const points: Mark[] = [
    { label: 'begin', at: 0 },
    ...sw.marks,
    { label: 'settled', at: settled },
  ];
  let best = { from: 'begin', to: 'settled', gap: 0 };
  for (let i = 1; i < points.length; i += 1) {
    const gap = points[i].at - points[i - 1].at;
    if (gap > best.gap) best = { from: points[i - 1].label, to: points[i].label, gap };
  }
  return best;
}

function finalize(sw: ActiveSwitch): void {
  if (sw.done) return;
  sw.done = true;
  if (sw.rafId !== null) cancelAnimationFrame(sw.rafId);
  if (sw.timeoutId !== null) clearTimeout(sw.timeoutId);
  if (active === sw) active = null;

  const settled = offset(sw);

  // Close the post-freeze race. onFrame measures a stall as the gap between two
  // frames, but a synchronous block running right up until finalize wins is
  // never seen by it: when the safety-net timer (or a preempting beginSwitch)
  // fires finalize while the thread is blocked, the trailing gap from the last
  // frame to now goes unmeasured. The pathology is a *continuous* freeze that
  // spans the whole switch — not one rAF fires, firstFrame stays null, and the
  // freeze would otherwise vanish as `maxGap 0 / stalls 0`. Synthesize that
  // trailing gap here, symmetric with onFrame: record it as a stall at
  // >= STALL_MS, and log it immediately at >= FREEZE_MS. Only count it as a
  // foreground freeze when the window stayed visible (hidden windows pause rAF).
  const tailStart = Math.round(sw.lastFrame - sw.t0);
  const tailGap = settled - tailStart;
  const hiddenDuringTail =
    sawHiddenSinceFrame || (typeof document !== 'undefined' && document.hidden);
  if (tailGap >= STALL_MS && !hiddenDuringTail) {
    if (tailGap > sw.maxGap) sw.maxGap = tailGap;
    sw.stalls.push({ at: tailStart, dur: tailGap });
    if (tailGap >= FREEZE_MS) {
      const continuous = sw.firstFrame === null;
      console.info(
        `[switch #${sw.id} +${tailStart}ms] main-thread frozen for ${tailGap}ms ` +
          `(foreground, ${continuous ? 'continuous — no frame fired the entire switch' : 'at switch tail'})`
      );
    }
  }

  const firstFrame = sw.firstFrame === null ? 'none (no frame fired)' : `+${sw.firstFrame}ms`;
  const stallCount = sw.stalls.length;
  const stallMax = sw.stalls.reduce((m, s) => Math.max(m, s.dur), 0);
  const stallTotal = sw.stalls.reduce((s, x) => s + x.dur, 0);
  const marksStr = sw.marks.map((m) => `${m.label}@+${m.at}ms`).join(', ');

  let line =
    `[switch #${sw.id} done] ${sw.from} → ${sw.to}` +
    ` | settled +${settled}ms` +
    ` | firstFrame ${firstFrame}` +
    ` | maxGap ${sw.maxGap}ms` +
    ` | stalls: count=${stallCount} max=${stallMax}ms total=${stallTotal}ms` +
    ` | mounts: ${fmtCounts(sw.mounts)}` +
    ` | unmounts: ${fmtCounts(sw.unmounts)}` +
    ` | sync: ${fmtSync(sw.sync)}` +
    ` | marks: [${marksStr}]`;
  // The largest span between two adjacent milestones localizes a freeze to a
  // named region; only surface it when it's freeze-sized so healthy switches
  // (where the biggest gap is just the quiet-settle tail) stay uncluttered.
  const bracket = largestMarkGap(sw, settled);
  if (bracket.gap >= FREEZE_MS) {
    line += ` | freezeBracket: ${bracket.from} → ${bracket.to} ${bracket.gap}ms`;
  }
  if (stallCount > 0) {
    const detail = sw.stalls.map((s) => `+${s.at}ms:${s.dur}ms`).join(', ');
    line += ` | stallDetail: [${detail}]`;
  }
  console.info(line);
}
