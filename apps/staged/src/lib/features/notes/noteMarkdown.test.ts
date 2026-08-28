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

  it('does not duplicate content that already starts with an H1', () => {
    expect(noteMarkdownWithTitle('Stored title', '# Existing title\n\nBody text.')).toBe(
      '# Existing title\n\nBody text.'
    );
  });

  it('leaves untitled note content unchanged', () => {
    expect(noteMarkdownWithTitle('', 'Body text.')).toBe('Body text.');
  });

  it('does not duplicate a title the content still carries as its first line', () => {
    expect(noteMarkdownWithTitle('Sub', '## Sub\n\nDetails.')).toBe('## Sub\n\nDetails.');
  });

  it('still prepends when the content opens with something else', () => {
    expect(noteMarkdownWithTitle('output.txt', '```\nlogs\n```')).toBe(
      '# output.txt\n\n```\nlogs\n```'
    );
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

  it('round-trips a note whose title came from the fallback', () => {
    const original = '- first item\n- second item';
    const { title, body } = splitNoteMarkdown(original);

    // The fallback body keeps the title line, so recombining must not add
    // another copy of it above the list.
    expect(noteMarkdownWithTitle(title, body)).toBe(original);
  });

  it('falls back to the first non-empty line when there is no H1', () => {
    expect(splitNoteMarkdown('\n\nJust a thought.\n\nMore.')).toEqual({
      title: 'Just a thought.',
      body: '\n\nJust a thought.\n\nMore.',
    });
  });

  it('strips heading markers from the fallback title', () => {
    expect(splitNoteMarkdown('## Overview\n\nDetails.').title).toBe('Overview');
  });

  it('clips a long fallback title', () => {
    const long = 'x'.repeat(120);

    expect(splitNoteMarkdown(long).title).toBe(`${'x'.repeat(80)}…`);
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
