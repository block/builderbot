import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MEDIA_AREA_PROPERTY, noteMediaAreaHeight, trackNoteMediaArea } from './noteMediaArea';

describe('noteMediaAreaHeight', () => {
  it('adds the room a content-sized dialog still has to grow', () => {
    // A short note: 300px of scroll area in a 400px dialog capped at 720px.
    expect(noteMediaAreaHeight(300, 400, 720)).toBe(620);
  });

  it('is the area itself once the dialog is at its maximum', () => {
    expect(noteMediaAreaHeight(620, 720, 720)).toBe(620);
  });

  it('gives the same height however tall the media makes the note', () => {
    // The media growing the area by 200px grows the dialog by as much.
    expect(noteMediaAreaHeight(300, 400, 720)).toBe(noteMediaAreaHeight(500, 600, 720));
  });

  it('is the area itself in a dialog with no maximum height', () => {
    expect(noteMediaAreaHeight(540, 600, null)).toBe(540);
  });

  it('never counts a dialog past its maximum as negative room', () => {
    expect(noteMediaAreaHeight(620, 730, 720)).toBe(620);
  });
});

describe('trackNoteMediaArea', () => {
  type Box = {
    clientHeight: number;
    offsetHeight: number;
    maxHeight: string;
    paddingTop: string;
    paddingBottom: string;
    dockOverlap: string;
  };
  let observers: { callback: () => void; targets: unknown[]; disconnected: boolean }[];
  let frames: (() => void)[];
  let resizeListeners: Set<() => void>;

  function element(box: Partial<Box>) {
    const properties = new Map<string, string>();
    return {
      clientHeight: 0,
      offsetHeight: 0,
      maxHeight: 'none',
      paddingTop: '0px',
      paddingBottom: '0px',
      dockOverlap: '',
      ...box,
      style: {
        setProperty: (name: string, value: string) => properties.set(name, value),
        removeProperty: (name: string) => properties.delete(name),
      },
      properties,
    };
  }
  type Fake = ReturnType<typeof element>;

  function runFrames() {
    const pending = frames;
    frames = [];
    for (const frame of pending) frame();
  }

  beforeEach(() => {
    observers = [];
    frames = [];
    resizeListeners = new Set();
    vi.stubGlobal(
      'ResizeObserver',
      class {
        entry: (typeof observers)[number];
        constructor(callback: () => void) {
          this.entry = { callback, targets: [], disconnected: false };
          observers.push(this.entry);
        }
        observe(target: unknown) {
          this.entry.targets.push(target);
        }
        disconnect() {
          this.entry.disconnected = true;
        }
      }
    );
    vi.stubGlobal('requestAnimationFrame', (frame: () => void) => frames.push(frame));
    vi.stubGlobal('cancelAnimationFrame', () => (frames = []));
    vi.stubGlobal('getComputedStyle', (el: Fake) => ({
      maxHeight: el.maxHeight,
      paddingTop: el.paddingTop,
      paddingBottom: el.paddingBottom,
      getPropertyValue: (name: string) =>
        name === '--selection-dock-overlap' ? el.dockOverlap : '',
    }));
    vi.stubGlobal('window', {
      addEventListener: (_: string, listener: () => void) => resizeListeners.add(listener),
      removeEventListener: (_: string, listener: () => void) => resizeListeners.delete(listener),
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function track(area: Fake, dialog: Fake) {
    return trackNoteMediaArea(area as unknown as HTMLElement, dialog as unknown as HTMLElement);
  }

  it('writes the area height straight away', () => {
    const area = element({ clientHeight: 300 });
    const dialog = element({ offsetHeight: 400, maxHeight: '720px' });
    track(area, dialog);
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('620px');
  });

  it('follows a resize on the next frame', () => {
    const area = element({ clientHeight: 300 });
    const dialog = element({ offsetHeight: 400, maxHeight: '720px' });
    track(area, dialog);
    expect(observers[0].targets).toEqual([area, dialog]);

    // The window shrinks: the dialog's maximum drops with it.
    dialog.maxHeight = '540px';
    for (const listener of resizeListeners) listener();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('620px');
    runFrames();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('440px');

    // A bottom bar takes 60px from the area of a dialog at its maximum.
    area.clientHeight = 480;
    dialog.offsetHeight = 540;
    observers[0].callback();
    runFrames();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('480px');
  });

  it('measures inside the area padding, as cqh does', () => {
    // 24px of padding each side, 34px more at the bottom for a safe area.
    const area = element({ clientHeight: 400, paddingTop: '24px', paddingBottom: '58px' });
    const dialog = element({ offsetHeight: 600 });
    track(area, dialog);
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('318px');
  });

  it('keeps the last height while the selection dock pads the area', () => {
    const area = element({ clientHeight: 400, paddingTop: '24px', paddingBottom: '24px' });
    const dialog = element({ offsetHeight: 600 });
    track(area, dialog);
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('352px');

    // A selection brings up the dock and 40px more padding: media keeps its size.
    area.paddingBottom = '64px';
    area.dockOverlap = '40px';
    observers[0].callback();
    runFrames();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('352px');

    // The window shrinks once the dock is gone again.
    area.paddingBottom = '24px';
    area.dockOverlap = '0px';
    area.clientHeight = 300;
    observers[0].callback();
    runFrames();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('252px');
  });

  it('measures once the dock leaves, even if no box resizes', () => {
    // A full-screen dialog: the dock's overlap fits inside the 34px safe-area
    // padding, so the dock coming and going changes no box.
    const area = element({ clientHeight: 400, paddingTop: '24px', paddingBottom: '58px' });
    const dialog = element({ offsetHeight: 600 });
    const tracker = track(area, dialog);
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('318px');

    // Nothing is owed yet, so a refresh measures nothing.
    tracker?.refresh();
    expect(frames).toHaveLength(0);

    // The window shrinks while the dock shows: that measurement is skipped.
    area.dockOverlap = '20px';
    area.clientHeight = 300;
    for (const listener of resizeListeners) listener();
    runFrames();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('318px');

    // The dock leaves without resizing anything; the refresh takes the
    // skipped measurement on the next frame.
    area.dockOverlap = '0px';
    tracker?.refresh();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('318px');
    runFrames();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('218px');

    // Then it is up to date again.
    tracker?.refresh();
    expect(frames).toHaveLength(0);
  });

  it('keeps the last height while the area is hidden', () => {
    const area = element({ clientHeight: 500 });
    const dialog = element({ offsetHeight: 600 });
    track(area, dialog);
    area.clientHeight = 0;
    observers[0].callback();
    runFrames();
    expect(area.properties.get(MEDIA_AREA_PROPERTY)).toBe('500px');
  });

  it('stops and clears the height on cleanup', () => {
    const area = element({ clientHeight: 500 });
    const dialog = element({ offsetHeight: 600 });
    track(area, dialog)?.stop();
    expect(observers[0].disconnected).toBe(true);
    expect(resizeListeners.size).toBe(0);
    expect(area.properties.has(MEDIA_AREA_PROPERTY)).toBe(false);
  });

  it('ignores a refresh once stopped, even with a measurement owed', () => {
    const area = element({ clientHeight: 400 });
    const dialog = element({ offsetHeight: 600 });
    const tracker = track(area, dialog);

    // A resize while the dock shows leaves a measurement owed.
    area.dockOverlap = '20px';
    for (const listener of resizeListeners) listener();
    runFrames();

    tracker?.stop();
    area.dockOverlap = '0px';
    tracker?.refresh();
    expect(frames).toHaveLength(0);
    expect(area.properties.has(MEDIA_AREA_PROPERTY)).toBe(false);
  });

  it('does nothing without both boxes', () => {
    expect(trackNoteMediaArea(undefined, null)).toBeUndefined();
    expect(observers).toHaveLength(0);
  });
});
