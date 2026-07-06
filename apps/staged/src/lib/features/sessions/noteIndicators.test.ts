import { describe, expect, it } from 'vitest';
import { splitAtNoteIndicator } from './noteIndicators';

describe('splitAtNoteIndicator', () => {
  it('cuts at a standalone horizontal rule before note content', () => {
    const split = splitAtNoteIndicator('Preamble\n---\n# My Note\nBody here.');

    expect(split).toEqual({ preamble: 'Preamble\n', hasNote: true });
  });

  it('does not detect a note without an indicator', () => {
    const text = 'Just some text without a horizontal rule.';

    expect(splitAtNoteIndicator(text)).toEqual({ preamble: text, hasNote: false });
  });

  it('cuts at an inline horizontal rule immediately before an H1', () => {
    const split = splitAtNoteIndicator(
      'I gathered enough context.---\n# Repo Purpose\nThis repo ships desktop tooling.'
    );

    expect(split).toEqual({ preamble: 'I gathered enough context.', hasNote: true });
  });

  it('cuts at inline suggested-next-steps before note markdown', () => {
    const split =
      splitAtNoteIndicator(`I focused the plan on the parser and tests.\`\`\`suggested-next-steps
{"suggestedNextCommitStep":"Fix note parsing","suggestedNextNoteStep":null}
\`\`\`
---
# Harden Note Detection
Strip metadata before scanning for the note separator.`);

    expect(split).toEqual({
      preamble: 'I focused the plan on the parser and tests.',
      hasNote: true,
    });
  });

  it('ignores inline horizontal rules that are not immediately followed by an H1', () => {
    const text = 'Two reasons:--- this session is read-only.';

    expect(splitAtNoteIndicator(text)).toEqual({ preamble: text, hasNote: false });
  });

  it('uses the first horizontal rule in a message', () => {
    const split = splitAtNoteIndicator(
      'Here is the format:\n---\n# <Title>\n<Body>\n\nNow here is my actual note:\n---\n# Real Title\nReal body.'
    );

    expect(split).toEqual({ preamble: 'Here is the format:\n', hasNote: true });
  });

  it('skips horizontal rules inside code fences', () => {
    const split = splitAtNoteIndicator(
      'Here is an example:\n```\n---\n# <Title>\n<Body>\n```\n---\n# Actual Note\nActual body.'
    );

    expect(split).toEqual({
      preamble: 'Here is an example:\n```\n---\n# <Title>\n<Body>\n```\n',
      hasNote: true,
    });
  });

  it('treats a trailing standalone horizontal rule as a streaming indicator', () => {
    const text = 'Drafting the note now.\n---';

    expect(splitAtNoteIndicator(text)).toEqual({ preamble: text, hasNote: false });
    expect(splitAtNoteIndicator(text, { streaming: true })).toEqual({
      preamble: 'Drafting the note now.\n',
      hasNote: true,
    });
  });

  it('treats an unterminated suggested-next-steps fence as a streaming indicator', () => {
    const text = 'Drafting the note now.```suggested-next-steps';

    expect(splitAtNoteIndicator(text)).toEqual({ preamble: text, hasNote: false });
    expect(splitAtNoteIndicator(text, { streaming: true })).toEqual({
      preamble: 'Drafting the note now.',
      hasNote: true,
    });
  });
});
