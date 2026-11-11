import { describe, expect, it } from 'vitest';
import {
  appendDraftQuote,
  composerDraftContent,
  composerDraftFromContent,
  composerDraftsEqual,
  emptyComposerDraft,
  hasQuotedParts,
  isComposerDraftEmpty,
  normalizeQuoteText,
  parseQuotedMessage,
  removeDraftQuote,
  serializeComposerDraft,
  type ComposerDraft,
} from './composerDraft';

const dataFlow = {
  quote: 'The cache is invalidated on every write.',
  headings: ['Architecture', 'Data flow'],
  reply: 'Could we batch those?',
};
const rollout = {
  quote: 'Ship behind a flag.',
  headings: ['Rollout'],
  reply: 'Which flag system?',
};

function review(): ComposerDraft {
  let draft: ComposerDraft = { quotes: [], text: 'A few comments:' };
  draft = appendDraftQuote(draft, dataFlow, 'q1');
  draft = { ...draft, text: 'And one more.' };
  draft = appendDraftQuote(draft, rollout, 'q2');
  return { ...draft, text: 'Thanks!' };
}

describe('appendDraftQuote', () => {
  it('moves the main input above the new quote and empties it', () => {
    const draft = appendDraftQuote({ quotes: [], text: 'Intro' }, dataFlow, 'q1');

    expect(draft).toEqual({
      quotes: [{ id: 'q1', before: 'Intro', ...dataFlow }],
      text: '',
    });
  });

  it('accumulates quotes in the order they were added', () => {
    const draft = review();

    expect(draft.quotes.map((quote) => [quote.id, quote.before])).toEqual([
      ['q1', 'A few comments:'],
      ['q2', 'And one more.'],
    ]);
    expect(draft.text).toBe('Thanks!');
  });

  it('copies the headings rather than sharing the caller’s array', () => {
    const headings = ['A'];
    const draft = appendDraftQuote(emptyComposerDraft(), { quote: 'x', headings, reply: '' });
    headings.push('B');

    expect(draft.quotes[0].headings).toEqual(['A']);
  });
});

describe('removeDraftQuote', () => {
  it('joins the text before a middle quote onto the next quote’s', () => {
    const draft = removeDraftQuote(review(), 'q1');

    expect(draft.quotes).toHaveLength(1);
    expect(draft.quotes[0]).toMatchObject({ id: 'q2', before: 'A few comments:\n\nAnd one more.' });
    expect(draft.text).toBe('Thanks!');
  });

  it('joins the text before the last quote onto the main input', () => {
    const draft = removeDraftQuote(review(), 'q2');

    expect(draft.quotes.map((quote) => quote.id)).toEqual(['q1']);
    expect(draft.text).toBe('And one more.\n\nThanks!');
  });

  it('keeps one side as-is when the other is empty', () => {
    const draft = removeDraftQuote(appendDraftQuote(emptyComposerDraft(), dataFlow, 'q1'), 'q1');

    expect(draft).toEqual({ quotes: [], text: '' });
  });

  it('leaves the draft alone for an unknown id', () => {
    const draft = review();

    expect(removeDraftQuote(draft, 'missing')).toBe(draft);
  });
});

describe('isComposerDraftEmpty', () => {
  it('counts a quote as content even with no reply', () => {
    expect(isComposerDraftEmpty({ quotes: [], text: '  \n' })).toBe(true);
    expect(
      isComposerDraftEmpty(
        appendDraftQuote(emptyComposerDraft(), { quote: 'x', headings: [], reply: '' })
      )
    ).toBe(false);
  });
});

describe('serializeComposerDraft', () => {
  it('writes each quote as a blockquote under its heading path, in draft order', () => {
    expect(serializeComposerDraft(review())).toBe(
      [
        'A few comments:',
        'Re: Architecture › Data flow\n> The cache is invalidated on every write.',
        'Could we batch those?',
        'And one more.',
        'Re: Rollout\n> Ship behind a flag.',
        'Which flag system?',
        'Thanks!',
      ].join('\n\n')
    );
  });

  it('quotes every line, blank ones included, and drops empty parts', () => {
    const draft = appendDraftQuote(
      emptyComposerDraft(),
      { quote: 'First line\n\nSecond paragraph', headings: [], reply: '   ' },
      'q1'
    );

    expect(serializeComposerDraft(draft)).toBe('> First line\n>\n> Second paragraph');
  });

  it('is the trimmed text for a draft without quotes', () => {
    expect(serializeComposerDraft({ quotes: [], text: '  hello  ' })).toBe('hello');
  });
});

describe('normalizeQuoteText', () => {
  it('trims, drops trailing spaces and collapses blank-line runs', () => {
    expect(normalizeQuoteText('  one  \r\n\r\n\r\n\ntwo \n')).toBe('one\n\ntwo');
  });
});

describe('persisted content', () => {
  it('stores a quote-free draft as its plain text, as before quotes existed', () => {
    expect(composerDraftContent({ quotes: [], text: 'just text' })).toBe('just text');
    expect(composerDraftFromContent({ value: 'just text' })).toEqual({
      quotes: [],
      text: 'just text',
    });
  });

  it('round-trips a draft with quotes, keeping the message as its text form', () => {
    const draft = review();
    const content = composerDraftContent(draft);

    expect(typeof content).toBe('object');
    if (typeof content === 'string') return;
    expect(content.value).toBe(serializeComposerDraft(draft));
    // Through JSON, as the store does.
    const restored = composerDraftFromContent(JSON.parse(JSON.stringify(content)));
    expect(composerDraftsEqual(restored, draft)).toBe(true);
  });

  it('falls back to the text form when the structure is unreadable', () => {
    expect(
      composerDraftFromContent({
        value: 'serialized',
        structured: { version: 99, quotes: [], text: '' },
      })
    ).toEqual({ quotes: [], text: 'serialized' });
    expect(
      composerDraftFromContent({
        value: 'serialized',
        structured: { version: 1, quotes: [{ id: 'q1', quote: 'x' }], text: '' },
      })
    ).toEqual({ quotes: [], text: 'serialized' });
  });

  it('is empty for a missing entry', () => {
    expect(composerDraftFromContent(null)).toEqual(emptyComposerDraft());
  });
});

describe('parseQuotedMessage', () => {
  it('reads back what serializeComposerDraft wrote', () => {
    const parts = parseQuotedMessage(serializeComposerDraft(review()));

    expect(parts).toEqual([
      { type: 'text', text: 'A few comments:' },
      { type: 'quote', quote: dataFlow.quote, headings: dataFlow.headings },
      { type: 'text', text: 'Could we batch those?\n\nAnd one more.' },
      { type: 'quote', quote: rollout.quote, headings: rollout.headings },
      { type: 'text', text: 'Which flag system?\n\nThanks!' },
    ]);
    expect(hasQuotedParts(parts)).toBe(true);
  });

  it('keeps multi-line quotes together, blank quote lines included', () => {
    expect(parseQuotedMessage('> one\n>\n> two\n\nreply')).toEqual([
      { type: 'quote', quote: 'one\n\ntwo', headings: [] },
      { type: 'text', text: 'reply' },
    ]);
  });

  it('leaves a plain message as one text part', () => {
    const parts = parseQuotedMessage('hello\nworld');

    expect(parts).toEqual([{ type: 'text', text: 'hello\nworld' }]);
    expect(hasQuotedParts(parts)).toBe(false);
  });

  it('does not read REPL prompts, unspaced markers or fenced code as quotes', () => {
    const message = ['>>> print(1)', '>not a quote', '```', '> inside a fence', '```'].join('\n');

    expect(hasQuotedParts(parseQuotedMessage(message))).toBe(false);
  });

  it('reads a Re: line only directly above a quote as its path', () => {
    expect(parseQuotedMessage('Re: something\n\n> quoted')).toEqual([
      { type: 'text', text: 'Re: something' },
      { type: 'quote', quote: 'quoted', headings: [] },
    ]);
  });

  it('leaves bare quote markers as text', () => {
    expect(parseQuotedMessage('Re: A\n>\n>')).toEqual([{ type: 'text', text: 'Re: A\n>\n>' }]);
  });
});
