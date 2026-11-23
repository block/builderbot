import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { WriteNoteCall } from '../noteToolCall';
import type { ToolCallViewModel } from '../toolCallViewModel';
import NoteToolDetails from './NoteToolDetails.svelte';

// The tool result is OutputSections' to render; these tests are about the
// note panel above it.
vi.mock('./OutputSections.svelte', () => import('../testStubs/EmptyStub.svelte'));

let details: ReturnType<typeof mount> | null = null;

function render(note: WriteNoteCall | null) {
  const viewModel = { metadata: { note } } as unknown as ToolCallViewModel;
  const target = document.createElement('div');
  document.body.appendChild(target);
  details = mount(NoteToolDetails, { target, props: { viewModel } });
  flushSync();
  return target;
}

const fields = (target: HTMLElement) =>
  [...target.querySelectorAll<HTMLElement>('.tool-field-list > span')].map((span) =>
    span.textContent?.trim()
  );

afterEach(() => {
  if (details) unmount(details);
  details = null;
  document.body.innerHTML = '';
});

describe('NoteToolDetails', () => {
  it('shows the suggested next steps of a call, which has no title', () => {
    const target = render({
      title: null,
      suggestedNextCommitStep: 'Expand the README',
      suggestedNextNoteStep: 'Plan the docs site',
    });

    expect(target.querySelector('.tool-note-panel')).not.toBeNull();
    expect(target.querySelector('.tool-note-title')).toBeNull();
    expect(fields(target)).toEqual([
      'Next commit',
      'Expand the README',
      'Next note',
      'Plan the docs site',
    ]);
  });

  it('shows the title of a call recorded with one above the steps', () => {
    const target = render({
      title: 'README.md: Sandbox repository purpose',
      suggestedNextCommitStep: 'Expand the README',
      suggestedNextNoteStep: 'Plan the docs site',
    });

    expect(target.querySelector('.tool-note-title')?.textContent).toBe(
      'README.md: Sandbox repository purpose'
    );
    expect(fields(target)).toEqual([
      'Next commit',
      'Expand the README',
      'Next note',
      'Plan the docs site',
    ]);
  });

  it('shows no body preview, toggle or "No note body" row', () => {
    const target = render({
      title: null,
      suggestedNextCommitStep: 'Expand the README',
      suggestedNextNoteStep: null,
    });

    expect(fields(target)).toEqual(['Next commit', 'Expand the README']);
    expect(target.querySelector('pre')).toBeNull();
    expect(target.querySelector('button')).toBeNull();
    expect(target.textContent).not.toContain('No note body');
  });

  it('lists only the step that was suggested', () => {
    const target = render({
      title: null,
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: 'Plan a fix for this bug',
    });

    expect(fields(target)).toEqual(['Next note', 'Plan a fix for this bug']);
  });

  it('shows a title that came with no steps, and no step list', () => {
    const target = render({
      title: 'Plan',
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: null,
    });

    expect(target.querySelector('.tool-note-title')?.textContent).toBe('Plan');
    expect(target.querySelector('.tool-field-list')).toBeNull();
  });

  it('renders no note panel for a call with no title and no steps', () => {
    const target = render({
      title: null,
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: null,
    });

    expect(target.querySelector('.tool-note-panel')).toBeNull();
  });
});
