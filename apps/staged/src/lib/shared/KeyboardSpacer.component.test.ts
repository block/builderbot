import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import KeyboardSpacer from './KeyboardSpacer.svelte';

// The spacer against the real visual viewport tracking. jsdom has no
// `visualViewport` and lays nothing out, so both are stood in with iOS 27.2
// Safari's numbers: a 714 px layout viewport, 384 px visible with the
// keyboard up.

const viewportState = vi.hoisted(() => ({ hasCoarsePointer: true }));
vi.mock('./viewport.svelte', () => ({ viewport: viewportState }));

class FakeVisualViewport extends EventTarget {
  height = 714;
  offsetTop = 0;
  scale = 1;
}

let visualViewport: FakeVisualViewport;
let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;
let search: HTMLInputElement;

function render(): void {
  component = mount(KeyboardSpacer, { target });
  flushSync();
}

function openKeyboard(): void {
  search.focus();
  visualViewport.height = 384;
  visualViewport.dispatchEvent(new Event('resize'));
  flushSync();
}

const spacer = () => target.querySelector<HTMLElement>('.keyboard-spacer');

beforeEach(() => {
  viewportState.hasCoarsePointer = true;
  visualViewport = new FakeVisualViewport();
  vi.stubGlobal('visualViewport', visualViewport);
  vi.spyOn(document.documentElement, 'clientHeight', 'get').mockReturnValue(714);
  // The first read waits for a frame; nothing here needs it.
  vi.stubGlobal('requestAnimationFrame', () => 1);
  vi.stubGlobal('cancelAnimationFrame', () => {});
  search = document.createElement('input');
  search.type = 'search';
  target = document.createElement('div');
  document.body.append(search, target);
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  flushSync();
  document.body.innerHTML = '';
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('KeyboardSpacer', () => {
  it('renders nothing without a keyboard', () => {
    render();
    expect(spacer()).toBeNull();

    // A focused input with nothing over the page.
    search.focus();
    visualViewport.dispatchEvent(new Event('resize'));
    flushSync();
    expect(spacer()).toBeNull();
  });

  it('is given what the keyboard covers', () => {
    render();
    openKeyboard();

    // The stylesheet takes the home indicator's inset off this.
    expect(spacer()?.style.getPropertyValue('--keyboard-inset')).toBe('330px');
    expect(spacer()?.getAttribute('aria-hidden')).toBe('true');
  });

  it('goes once the keyboard has left', () => {
    render();
    openKeyboard();
    expect(spacer()).not.toBeNull();

    search.blur();
    visualViewport.height = 714;
    visualViewport.dispatchEvent(new Event('resize'));
    flushSync();

    expect(spacer()).toBeNull();
  });

  it('does not track the viewport without a touch screen', () => {
    viewportState.hasCoarsePointer = false;
    render();
    openKeyboard();

    expect(spacer()).toBeNull();
  });
});
