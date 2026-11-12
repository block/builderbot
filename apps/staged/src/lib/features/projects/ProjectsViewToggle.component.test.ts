// Mounts the real toggle over bits-ui: the assertions are on which segment
// reads as pressed after a click, and on what the parent hears.
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ProjectsViewToggle from './ProjectsViewToggle.svelte';

let component: ReturnType<typeof mount> | null = null;

function mountToggle(value: 'grid' | 'board') {
  const onChange = vi.fn();
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectsViewToggle, { target, props: { value, onChange } });
  flushSync();
  return { target, onChange };
}

function segment(root: ParentNode, label: string): HTMLButtonElement {
  return root.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
}

function pressed(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLButtonElement>('button[data-state="on"]')].map((el) =>
    el.getAttribute('aria-label')!
  );
}

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('ProjectsViewToggle', () => {
  it('reports a pick of the other segment', () => {
    const { target, onChange } = mountToggle('grid');
    expect(pressed(target)).toEqual(['Grid']);

    segment(target, 'Board').click();
    flushSync();

    expect(onChange).toHaveBeenCalledWith('board');
    // The parent owns the value: until it passes 'board' back, Grid stays.
    expect(pressed(target)).toEqual(['Grid']);
  });

  it('keeps the pressed segment pressed when it is clicked again', () => {
    const { target, onChange } = mountToggle('grid');

    // bits-ui would clear a single group here; nothing may hear or show that.
    segment(target, 'Grid').click();
    flushSync();

    expect(onChange).not.toHaveBeenCalled();
    expect(pressed(target)).toEqual(['Grid']);
  });

  it('starts on the mode it is given', () => {
    const { target } = mountToggle('board');
    expect(pressed(target)).toEqual(['Board']);
  });
});
