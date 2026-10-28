// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { firstTabbableAfter, firstTabbableWithin, lastTabbableBefore } from './tabbable';

describe('lastTabbableBefore', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('returns the last tabbable element preceding the boundary', () => {
    document.body.innerHTML = `
      <div class="top-bar">
        <button id="back">Back</button>
        <span id="widget" tabindex="0">Widget</span>
        <button id="disabled" disabled>Nope</button>
        <span id="skipped" tabindex="-1">Nope</span>
      </div>
      <div id="workspace"><button id="inside">Inside</button></div>
      <div id="host"><button id="after">After</button></div>
    `;

    const found = lastTabbableBefore(document.getElementById('workspace')!);

    expect(found?.id).toBe('widget');
  });

  it('skips elements inside an inert subtree', () => {
    document.body.innerHTML = `
      <button id="back">Back</button>
      <div inert><button id="inert-child">Nope</button></div>
      <div id="workspace"></div>
    `;

    expect(lastTabbableBefore(document.getElementById('workspace')!)?.id).toBe('back');
  });

  it('returns null when nothing tabbable precedes the boundary', () => {
    document.body.innerHTML = `
      <div id="workspace"><button>Inside</button></div>
      <button id="after">After</button>
    `;

    expect(lastTabbableBefore(document.getElementById('workspace')!)).toBeNull();
  });

  it('skips elements inside an aria-hidden subtree', () => {
    // A covered layer carries aria-hidden rather than inert (inert restyles
    // the whole subtree), so its controls must be skipped explicitly.
    document.body.innerHTML = `
      <button id="back">Back</button>
      <div class="workspace-layer" aria-hidden="true"><button id="hidden-child">Nope</button></div>
      <div id="workspace"></div>
    `;

    expect(lastTabbableBefore(document.getElementById('workspace')!)?.id).toBe('back');
  });
});

describe('firstTabbableAfter', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('returns the first tabbable element following the boundary and its subtree', () => {
    document.body.innerHTML = `
      <button id="back">Back</button>
      <div id="hidden" tabindex="-1">
        <button id="inside">Inside</button>
        <button id="inside-last">Inside last</button>
      </div>
      <div id="next" tabindex="-1">
        <button id="disabled" disabled>Nope</button>
        <span id="skipped" tabindex="-1">Nope</span>
        <button id="first-after">First after</button>
        <button id="second-after">Second after</button>
      </div>
    `;

    expect(firstTabbableAfter(document.getElementById('hidden')!)?.id).toBe('first-after');
  });

  it('wraps to the first tabbable outside the boundary when nothing follows', () => {
    document.body.innerHTML = `
      <div class="top-bar">
        <span id="skipped" tabindex="-1">Nope</span>
        <button id="first">First</button>
      </div>
      <div id="hidden" tabindex="-1"><button id="inside">Inside</button></div>
    `;

    expect(firstTabbableAfter(document.getElementById('hidden')!)?.id).toBe('first');
  });

  it('returns null when the boundary holds the only tabbable content', () => {
    document.body.innerHTML = `
      <div id="hidden" tabindex="-1"><button id="inside">Inside</button></div>
    `;

    expect(firstTabbableAfter(document.getElementById('hidden')!)).toBeNull();
  });

  it('skips aria-hidden and inert subtrees', () => {
    document.body.innerHTML = `
      <div id="hidden" tabindex="-1"><button id="inside">Inside</button></div>
      <div aria-hidden="true"><button id="covered">Nope</button></div>
      <div inert><button id="inert-child">Nope</button></div>
      <button id="after">After</button>
    `;

    expect(firstTabbableAfter(document.getElementById('hidden')!)?.id).toBe('after');
  });
});

describe('firstTabbableWithin', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('returns the first tabbable descendant, skipping untabbable ones', () => {
    document.body.innerHTML = `
      <button id="outside">Outside</button>
      <div id="host" tabindex="-1">
        <button id="disabled" disabled>Nope</button>
        <span id="skipped" tabindex="-1">Nope</span>
        <div inert><button id="inert-child">Nope</button></div>
        <input id="first" />
        <button id="second">Second</button>
      </div>
    `;

    expect(firstTabbableWithin(document.getElementById('host')!)?.id).toBe('first');
  });

  it('returns null when the root holds nothing tabbable', () => {
    document.body.innerHTML = `
      <div id="host" tabindex="-1"><span>Text</span></div>
      <button id="after">After</button>
    `;

    expect(firstTabbableWithin(document.getElementById('host')!)).toBeNull();
  });

  it('skips descendants inside an aria-hidden subtree', () => {
    document.body.innerHTML = `
      <div id="host" tabindex="-1">
        <div class="workspace-layer" aria-hidden="true"><button id="covered">Nope</button></div>
        <div class="workspace-layer"><button id="shown">Shown</button></div>
      </div>
    `;

    expect(firstTabbableWithin(document.getElementById('host')!)?.id).toBe('shown');
  });
});
