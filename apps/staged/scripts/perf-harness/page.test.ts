// @vitest-environment jsdom
//
// The scripts the driver runs inside the app's page (`driver/ui.mjs`),
// against markup shaped like the app's: branch cards that carry their
// branch id and mount their timeline rows only when near the visible part
// of the scroll area.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { desktopLoadState, openDesktopFrontend } from './driver/frontend.mjs';
import * as ui from './driver/ui.mjs';

const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor;
const run = (body: string) => new AsyncFunction(body)();

/** A branch card, with its session's row when `row` is given. */
function card(branchId: string, row?: { tag: string; subtitle?: string }) {
  const el = document.createElement('div');
  el.className = 'branch-card';
  el.dataset.branchId = branchId;
  if (row) el.append(timelineRow(row.tag, row.subtitle));
  return el;
}

function timelineRow(tag: string, subtitle = '') {
  const el = document.createElement('div');
  el.className = 'timeline-row';
  el.innerHTML = `<div class="timeline-title">Load test pf-tag=${tag} pf-profile=background</div>
    <div class="timeline-meta"><span class="meta-item">${subtitle}</span><span class="meta-item">2m</span></div>
    <button aria-label="View session"></button>`;
  return el;
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('reading the rows', () => {
  const sessions = [
    { tag: 's0p0b1', branchId: 'b-1' },
    { tag: 's0p0b10', branchId: 'b-10' },
    { tag: 's0p0b2', branchId: 'b-2' },
    { tag: 's0p0b3', branchId: 'b-3' },
  ];

  it('says for each session whether its row is mounted and which stamp it shows', async () => {
    document.body.append(
      card('b-1', { tag: 's0p0b1', subtitle: 'pfm-xs0p0b1-12-1700000000120 the quick brown fox' }),
      // A tag that begins with another's: neither is taken for the other.
      card('b-10', { tag: 's0p0b10', subtitle: 'Run pfm-ts0p0b10-40-1700000000400' }),
      // Mounted, nothing shown yet.
      card('b-2', { tag: 's0p0b2' }),
      // Scrolled away: the card is there and its timeline is not.
      card('b-3')
    );
    const before = Date.now();
    const { at, rows } = await run(ui.rows(sessions));
    expect(at).toBeGreaterThanOrEqual(before);
    expect(rows).toEqual([
      { tag: 's0p0b1', card: true, row: true, kind: 'x', seq: 12, sent: 1700000000120 },
      { tag: 's0p0b10', card: true, row: true, kind: 't', seq: 40, sent: 1700000000400 },
      { tag: 's0p0b2', card: true, row: true, kind: null, seq: null, sent: null },
      { tag: 's0p0b3', card: true, row: false, kind: null, seq: null, sent: null },
    ]);
  });

  it('reads the live copy of a card when a parked copy of the page is still in the DOM', async () => {
    const parked = document.createElement('div');
    parked.setAttribute('aria-hidden', 'true');
    parked.append(card('b-1', { tag: 's0p0b1', subtitle: 'pfm-xs0p0b1-3-1700000000030 old' }));
    document.body.append(
      parked,
      card('b-1', { tag: 's0p0b1', subtitle: 'pfm-xs0p0b1-9-1700000000090 new' })
    );
    const { rows } = await run(ui.rows(sessions.slice(0, 1)));
    expect(rows[0].seq).toBe(9);
    // Under an open dialog every copy is hidden, and the one there is still read.
    document.body.lastElementChild!.remove();
    expect((await run(ui.rows(sessions.slice(0, 1)))).rows[0].seq).toBe(3);
  });

  it('reads the open chat’s text stamps, each once, and only this session’s', async () => {
    expect(await run(ui.chatStamps('s0p0b1'))).toBeNull();
    const dialog = document.createElement('div');
    dialog.setAttribute('role', 'dialog');
    dialog.innerHTML = `<div class="message-row">pfm-xs0p0b1-1-1700000000010 a
      pfm-xs0p0b1-2-1700000000020 b</div>
      <div class="message-row">Run pfm-ts0p0b1-3-1700000000030</div>
      <div class="message-row">pfm-xs0p0b1-2-1700000000020 b again pfm-xs0p0b10-7-1700000000070</div>`;
    document.body.append(dialog);
    expect(await run(ui.chatStamps('s0p0b1'))).toEqual([
      [1, 1700000000010],
      [2, 1700000000020],
    ]);
  });
});

describe('opening a project with more sessions than fit', () => {
  it('scrolls to the session’s card, which is what mounts its row', async () => {
    const far = card('b-far');
    const scrolled: string[] = [];
    // The app mounts a card's timeline when the card comes near the visible
    // part of the scroll area; here, when it is scrolled to.
    Element.prototype.scrollIntoView = function (this: HTMLElement) {
      scrolled.push(this.dataset.branchId ?? this.className);
      if (this === far && !far.querySelector('.timeline-row')) far.append(timelineRow('s0p0b0'));
    };
    document.body.append(card('b-near', { tag: 's0p0b24' }), far);

    const opened = await run(ui.openProject('Perf viewed 0', { branchId: 'b-far', tag: 's0p0b0' }));
    expect(scrolled[0]).toBe('b-far');
    expect(opened.rows).toBe(2);
    expect((await run(ui.rows([{ tag: 's0p0b0', branchId: 'b-far' }]))).rows[0].row).toBe(true);
  });

  it('opens the project first when its cards are not on the page', async () => {
    Element.prototype.scrollIntoView = () => {};
    const name = document.createElement('span');
    name.textContent = 'Perf viewed 1';
    name.addEventListener('click', () => document.body.append(card('b-0', { tag: 's1p0b0' })));
    document.body.append(name);
    await run(ui.openProject('Perf viewed 1', { branchId: 'b-0', tag: 's1p0b0' }));
    expect(document.querySelectorAll('.branch-card')).toHaveLength(1);
  });
});

describe('loading the desktop webview', () => {
  const REFUSAL = 'perf-harness: this port takes the run’s token';
  const PROBE = readFileSync(join(import.meta.dirname, 'driver', 'probe.js'), 'utf8');
  type Page = Window & {
    __stagedPerf?: unknown;
    __stagedPerfLeaving?: boolean;
    __reload?: () => void;
  };
  const page = window as Page;

  /**
   * The app's side of the harness routes, with this jsdom window as the
   * webview. A reload replaces the document `loadsAfter` scripts later,
   * with `next`: until then the old one is still what a script finds.
   */
  function webview({ next, loadsAfter = 0 }: { next: string; loadsAfter?: number }) {
    const results: string[] = [];
    let countdown: number | null = null;
    vi.stubGlobal('fetch', async (_url: string, init: { body: string }) => {
      results.push(init.body);
      return { ok: true };
    });
    page.__reload = () => {
      countdown = loadsAfter;
    };
    const navigate = () => {
      // A new document has a new window: nothing set on the old one is there.
      delete page.__stagedPerf;
      delete page.__stagedPerfLeaving;
      document.body.textContent = next;
    };
    return {
      token: 'the-run-token',
      evalInWebviews: async (script: string) => {
        if (countdown !== null && countdown-- <= 0) {
          countdown = null;
          navigate();
        }
        // The probe needs a real page; the load only asks that it is there.
        if (script === PROBE) page.__stagedPerf ??= {};
        else (0, eval)(script.replace('location.reload();', 'window.__reload();'));
        return ['main'];
      },
      takeResults: async () => results.splice(0),
    };
  }

  /** Run the driver's timers until `promise` settles; resolves to its outcome. */
  async function outcome(promise: Promise<unknown>) {
    let settled: { value?: unknown; error?: Error } | undefined;
    promise.then(
      (value) => (settled = { value }),
      (error) => (settled = { error })
    );
    while (!settled) await vi.advanceTimersByTimeAsync(250);
    return settled;
  }

  beforeEach(() => {
    vi.useFakeTimers();
    // The webview's first document: it was loaded without the token.
    document.body.textContent = REFUSAL;
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    for (const name of ['__stagedPerf', '__stagedPerfLeaving', '__reload'] as const)
      delete page[name];
    for (const name of ['hidden', 'visibilityState'])
      delete (document as unknown as Record<string, unknown>)[name];
    localStorage.clear();
  });

  it('says the app refused the page when the reload is refused too, and does not wait it out', async () => {
    const api = webview({ next: REFUSAL });
    const frontend = await openDesktopFrontend({ api, localStorage: {} });
    const started = Date.now();
    const { error } = await outcome(frontend.reload());
    expect(error?.message).toBe(
      'the app refused the page: the webview did not send the run’s token'
    );
    // Seconds, not the two minutes a page that never loads is given.
    expect(Date.now() - started).toBeLessThan(10_000);
  });

  it('does not take the refusal it reloads from for the answer to the reload', async () => {
    // The first document, a refusal, is still there for a few scripts.
    const api = webview({ next: 'Staged', loadsAfter: 6 });
    const frontend = await openDesktopFrontend({ api, localStorage: { 'some:switch': 'on' } });
    const { error } = await outcome(frontend.reload());
    expect(error).toBeUndefined();
    expect(document.body.textContent).toBe('Staged');
    expect(document.cookie).toContain('the-run-token');
    expect(localStorage.getItem('some:switch')).toBe('on');
    // The page is shown as visible, which is the reload's last step.
    expect(document.hidden).toBe(false);
  });

  it('reads where the load stands from the document', async () => {
    const state = () => run(desktopLoadState);
    // The refusal the reload is made from, marked as on its way out.
    page.__stagedPerfLeaving = true;
    page.__stagedPerf = {};
    expect(await state()).toBeNull();
    // A document of its own that is a refusal.
    delete page.__stagedPerfLeaving;
    expect(await state()).toBe('refused');
    // The app's page, before and after the probe is in.
    document.body.textContent = 'Staged';
    delete page.__stagedPerf;
    expect(await state()).toBeNull();
    page.__stagedPerf = {};
    expect(await state()).toBe('ready');
  });
});
