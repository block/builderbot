// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { lastTabbableBefore } from './tabbable';

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
});
