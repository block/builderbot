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

describe('splitNoteMarkdown with a first line that is not a title', () => {
  // Markdown whose meaning is its markup: as a one-line plain-text title it
  // would read as syntax, so the note is Untitled and the line stays put.
  const notTitles = [
    ['an image', '![Screenshot](shot.png)'],
    ['a link', '[The docs](https://example.com)'],
    ['a link inside a sentence', 'Follow [the docs](https://example.com) first'],
    ['a heading holding a link', '# [The docs](https://example.com)'],
    ['a bare URL', 'https://example.com/page'],
    ['a URL the serializer escaped', 'Read this https\\://example.com'],
    ['a bullet', '- first item'],
    ['a task item', '- [ ] first item'],
    ['a numbered item', '1. first item'],
    ['a quote', '> quoted'],
    ['a code fence', '```ts'],
    ['a table row', '| a | b |'],
    ['a rule', '---'],
    ['raw HTML', '<div class="x">'],
  ] as const;

  for (const [label, firstLine] of notTitles) {
    it(`leaves ${label} in the body and names the note Untitled`, () => {
      expect(splitNoteMarkdown(`${firstLine}\n\nRest.`)).toEqual({
        title: UNTITLED_NOTE_TITLE,
        body: `${firstLine}\n\nRest.`,
      });
    });
  }

  it('settles after one reopen instead of losing or repeating the line', () => {
    const typed = '![Screenshot](shot.png)\n\nRest.';
    const saved = splitNoteMarkdown(typed);

    // Reopening puts a real title line above the image, and saving again reads
    // that line rather than taking a second pass at the image.
    const reopened = noteMarkdownWithTitle(saved.title, saved.body);
    expect(reopened).toBe(`# ${UNTITLED_NOTE_TITLE}\n\n${typed}`);
    expect(splitNoteMarkdown(reopened)).toEqual(saved);
  });

  it('still takes ordinary titles, including decorated ones', () => {
    expect(splitNoteMarkdown('**Release** plan\n\nShip.').title).toBe('**Release** plan');
    expect(splitNoteMarkdown('Notes [draft] for v2\n\nShip.').title).toBe('Notes [draft] for v2');
  });
});

describe('renderNoteMarkdown', () => {
  it('uses the shared markdown renderer', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain('<pre class="markdown-diagram-source markdown-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
  });
});
