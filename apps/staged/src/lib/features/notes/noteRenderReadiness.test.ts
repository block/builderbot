import { describe, expect, it } from 'vitest';
import { noteRenderInputsReady } from './noteRenderReadiness';

const ready = {
  mediaDimensionsKnown: true,
  needsPikchr: false,
  pikchrRendererLoaded: false,
  pikchrRendererLoadFailed: false,
};

describe('noteRenderInputsReady', () => {
  it('renders a note without diagrams once its image dimensions are known', () => {
    expect(noteRenderInputsReady(ready)).toBe(true);
    expect(noteRenderInputsReady({ ...ready, mediaDimensionsKnown: false })).toBe(false);
  });

  it('waits for the pikchr renderer when the note has a pikchr fence', () => {
    // Rendering with the fence's placeholder and again with the renderer would
    // rebuild the note mid-read, remounting any video the reader had started.
    expect(noteRenderInputsReady({ ...ready, needsPikchr: true })).toBe(false);
    expect(noteRenderInputsReady({ ...ready, needsPikchr: true, pikchrRendererLoaded: true })).toBe(
      true
    );
  });

  it('renders once with the fallback when the pikchr renderer fails to load', () => {
    expect(
      noteRenderInputsReady({ ...ready, needsPikchr: true, pikchrRendererLoadFailed: true })
    ).toBe(true);
  });

  it('still waits for image dimensions when the renderer is in', () => {
    expect(
      noteRenderInputsReady({
        mediaDimensionsKnown: false,
        needsPikchr: true,
        pikchrRendererLoaded: true,
        pikchrRendererLoadFailed: false,
      })
    ).toBe(false);
  });
});
