import { describe, expect, it } from 'vitest';

import {
  UNTITLED_NOTE_TITLE,
  canBeNoteTitleLine,
  canBeNoteTitleText,
  noteMarkdownWithTitle,
  renderNoteMarkdown,
  splitNoteMarkdown,
  unescapeMarkdown,
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

describe('unescapeMarkdown', () => {
  it('consumes the escaped character along with its backslash', () => {
    // A lookahead that deleted the backslash alone would re-examine the second
    // half of a `\\` pair and eat that too, losing every backslash typed.
    expect(unescapeMarkdown('a\\\\\\*b')).toBe('a\\*b');
    expect(unescapeMarkdown('back\\\\\\slash')).toBe('back\\\\slash');
  });

  it('only undoes escapes CommonMark defines', () => {
    // The 32 ASCII punctuation characters, and nothing else: a backslash before
    // a letter or non-ASCII punctuation is literal, and the serializer knows it
    // and leaves it alone.
    expect(unescapeMarkdown('\\—dash \\word')).toBe('\\—dash \\word');
    expect(unescapeMarkdown('\\!\\"\\#\\$\\%\\&\\\'\\(\\)\\*\\+\\,\\-\\.\\/')).toBe(
      '!"#$%&\'()*+,-./'
    );
    expect(unescapeMarkdown('\\:\\;\\<\\=\\>\\?\\@\\[\\\\\\]\\^\\_\\`\\{\\|\\}\\~')).toBe(
      ':;<=>?@[\\]^_`{|}~'
    );
  });
});

describe('splitNoteMarkdown with a title the serializer escaped', () => {
  // The title column is plain text — the timeline row and `#note:` labels render
  // it as-is — but the line it comes from was written for a markdown parser.
  const escaped = [
    ['an identifier', '# snake\\_case\\_name', 'snake_case_name'],
    ['brackets', '# Plan \\[draft] v2', 'Plan [draft] v2'],
    ['an asterisk', '# Use \\* for wildcards', 'Use * for wildcards'],
    ['an ampersand', '# AT\\&T outage', 'AT&T outage'],
    ['a trailing hash', '# trailing hash \\#', 'trailing hash #'],
    ['an angle bracket', '# \\<not html>', '<not html>'],
  ] as const;

  for (const [label, firstLine, title] of escaped) {
    it(`stores ${label} as the text a reader sees`, () => {
      expect(splitNoteMarkdown(`${firstLine}\n\nRest.`)).toEqual({ title, body: 'Rest.' });
    });
  }

  it('keeps a backslash the user typed', () => {
    // Typed as `a\*b`, the line serializes to `a\\\*b`: the backslash is escaped
    // and so is the asterisk behind it.
    expect(splitNoteMarkdown('# a\\\\\\*b').title).toBe('a\\*b');
    expect(splitNoteMarkdown('# 50\\\\% off').title).toBe('50\\% off');
    expect(splitNoteMarkdown('# \\—dash').title).toBe('\\—dash');
  });

  it('reads the heading marker before unescaping, not after', () => {
    // `\# Heading` is a paragraph whose visible text opens with a `#`. Unescaping
    // first would take that for a marker and drop a character the user can see.
    expect(splitNoteMarkdown('\\# Heading\n\nRest.')).toEqual({
      title: '# Heading',
      body: 'Rest.',
    });
  });

  it("unescapes a dropped file's own heading too", () => {
    expect(splitNoteMarkdown('# snake\\_case\\_name\n\nHow to build it.', 'README').title).toBe(
      'snake_case_name'
    );
  });

  it('is a fixed point: a plain title survives the next round trip', () => {
    const title = 'snake_case_name';
    const body = 'Details.';

    expect(splitNoteMarkdown(noteMarkdownWithTitle(title, body))).toEqual({ title, body });
  });
});

describe('canBeNoteTitleLine and canBeNoteTitleText', () => {
  // The editor asks about a parsed block's text, the save path about the line the
  // serializer wrote for it. Both halves promise to agree, which they only do if
  // the escapes come off exactly once, on the side that has them.
  it('reject a bullet in either spelling', () => {
    expect(canBeNoteTitleText('- item')).toBe(false);
    expect(canBeNoteTitleLine('\\- item')).toBe(false);
  });

  it('accept a line that only looks like one', () => {
    // Visible text `\- item`: a literal backslash, so not a bullet at all.
    expect(canBeNoteTitleText('\\- item')).toBe(true);
    expect(canBeNoteTitleLine('\\\\- item')).toBe(true);
  });
});

describe('splitNoteMarkdown with a fallback title', () => {
  // The drag-drop writer already has a name for the note — the file's — so it
  // only gives it up to the document's own H1.
  it("takes the document's own H1 over the fallback", () => {
    expect(splitNoteMarkdown('# Project\n\nHow to build it.', 'README')).toEqual({
      title: 'Project',
      body: 'How to build it.',
    });
  });

  it('round-trips a dropped file that names itself', () => {
    const file = '# Project\n\nHow to build it.';
    const { title, body } = splitNoteMarkdown(file, 'README');

    // The heading is stored once, in the title column, so the viewer shows one.
    expect(noteMarkdownWithTitle(title, body)).toBe(file);
  });

  it('keeps the fallback when the first line is ordinary text', () => {
    const log = '12:00 boot\n12:01 ready';

    expect(splitNoteMarkdown(log, 'server')).toEqual({ title: 'server', body: log });
  });

  it('keeps the fallback for a heading below H1', () => {
    // `## Overview` is a section of the document, not the name of it.
    expect(splitNoteMarkdown('## Overview\n\nDetails.', 'notes')).toEqual({
      title: 'notes',
      body: '## Overview\n\nDetails.',
    });
  });

  it('keeps the fallback when the H1 could not be a title', () => {
    const doc = '# [The docs](https://example.com)\n\nRest.';

    expect(splitNoteMarkdown(doc, 'links')).toEqual({ title: 'links', body: doc });
  });

  it('names an empty file after itself rather than Untitled', () => {
    expect(splitNoteMarkdown('  \n\n', 'empty')).toEqual({ title: 'empty', body: '' });
  });
});

describe('renderNoteMarkdown', () => {
  it('uses the shared markdown renderer', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain('<pre class="markdown-diagram-source markdown-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
  });
});
