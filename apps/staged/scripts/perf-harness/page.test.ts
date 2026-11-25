// @vitest-environment jsdom
//
// The scripts the driver runs inside the app's page (`driver/ui.mjs`),
// against markup shaped like the app's: branch cards that carry their
// branch id and mount their timeline rows only when near the visible part
// of the scroll area.

import { afterEach, describe, expect, it } from 'vitest';

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
