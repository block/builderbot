import { describe, expect, it } from 'vitest';

import {
  UNTITLED_NOTE_TITLE,
  noteMarkdownWithTitle,
  renderNoteMarkdown,
  splitNoteMarkdown,
} from './noteMarkdown';

describe('noteMarkdownWithTitle', () => {
  it('prepends the note title as a markdown H1', () => {
    expect(noteMarkdownWithTitle('Investigation notes', 'Body text.')).toBe(
      '# Investigation notes\n\nBody text.'
    );
  });

  it('renders a title-only note as a markdown H1', () => {
    expect(noteMarkdownWithTitle('Standalone title', '')).toBe('# Standalone title');
  });

  it('prepends the stored title even when the body opens with its own heading', () => {
    // Hiding the title here would show the wrong one and, on the next save,
    // store the body's heading as the note's title.
    expect(noteMarkdownWithTitle('Stored title', '# Section\n\nBody text.')).toBe(
      '# Stored title\n\n# Section\n\nBody text.'
    );
  });

  it('leaves untitled note content unchanged', () => {
    expect(noteMarkdownWithTitle('', 'Body text.')).toBe('Body text.');
  });
});

describe('splitNoteMarkdown', () => {
  it('takes the leading H1 as the title and the rest as the body', () => {
    expect(splitNoteMarkdown('# Release plan\n\nShip on Friday.')).toEqual({
      title: 'Release plan',
      body: 'Ship on Friday.',
    });
  });

  it('handles a note that is only a title', () => {
    expect(splitNoteMarkdown('# Release plan')).toEqual({ title: 'Release plan', body: '' });
  });

  it('round-trips with noteMarkdownWithTitle', () => {
    const { title, body } = splitNoteMarkdown('# Release plan\n\nShip on Friday.');

    expect(noteMarkdownWithTitle(title, body)).toBe('# Release plan\n\nShip on Friday.');
  });

  it('keeps a body that opens with its own heading out of the title', () => {
    const original = '# Release plan\n\n# Risks\n\nShip on Friday.';
    const { title, body } = splitNoteMarkdown(original);

    expect(title).toBe('Release plan');
    expect(body).toBe('# Risks\n\nShip on Friday.');
    expect(noteMarkdownWithTitle(title, body)).toBe(original);
  });

  it('takes the first line as the title when it is not a heading', () => {
    expect(splitNoteMarkdown('\n\nJust a thought.\n\nMore.')).toEqual({
      title: 'Just a thought.',
      body: 'More.',
    });
  });

  it('strips heading markers from the title', () => {
    expect(splitNoteMarkdown('## Overview\n\nDetails.')).toEqual({
      title: 'Overview',
      body: 'Details.',
    });
  });

  it('keeps a long title whole', () => {
    const long = 'x'.repeat(120);

    // Clipping the title would drop the rest of the line: the body no longer
    // holds it, so the note would lose text on every save.
    expect(splitNoteMarkdown(long)).toEqual({ title: long, body: '' });
  });

  it('names a note with nothing usable in it', () => {
    expect(splitNoteMarkdown('   \n\n  ').title).toBe(UNTITLED_NOTE_TITLE);
  });
});

describe('renderNoteMarkdown', () => {
  it('uses the shared markdown renderer', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain('<pre class="markdown-diagram-source markdown-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
  });
});
