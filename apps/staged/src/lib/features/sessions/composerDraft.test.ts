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
  restoreDraftQuote,
  serializeComposerDraft,
  setDraftQuoteReply,
  summarizeComposerDraft,
  takeDraftQuote,
  type ComposerDraft,
  type NewDraftQuote,
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

describe('takeDraftQuote and restoreDraftQuote', () => {
  it('puts a middle quote back in its place and splits the merged text again', () => {
    const taken = takeDraftQuote(review(), 'q1')!;
    expect(taken.draft).toEqual(removeDraftQuote(review(), 'q1'));
    expect(taken.removed).toMatchObject({
      index: 0,
      unmerged: 'And one more.',
      merged: 'A few comments:\n\nAnd one more.',
    });

    expect(restoreDraftQuote(taken.draft, taken.removed)).toEqual(review());
  });

  it('leaves a draft that still holds the block alone, so a second composer does not double it', () => {
    const taken = takeDraftQuote(review(), 'q1')!;
    const untouched = review();

    expect(restoreDraftQuote(untouched, taken.removed)).toBe(untouched);
  });

  it('puts the last quote back before the main input', () => {
    const taken = takeDraftQuote(review(), 'q2')!;
    expect(taken.draft.text).toBe('And one more.\n\nThanks!');

    expect(restoreDraftQuote(taken.draft, taken.removed)).toEqual(review());
  });

  it('keeps text edited since the removal whole, and brings the quote back without its lead-in', () => {
    const taken = takeDraftQuote(review(), 'q1')!;
    const edited = {
      ...taken.draft,
      quotes: [{ ...taken.draft.quotes[0], before: 'Rewritten while the quote was out.' }],
    };

    const restored = restoreDraftQuote(edited, taken.removed);

    expect(restored.quotes.map((q) => q.id)).toEqual(['q1', 'q2']);
    expect(restored.quotes[0]).toMatchObject({ before: '', reply: dataFlow.reply });
    expect(restored.quotes[1].before).toBe('Rewritten while the quote was out.');
    expect(restored.text).toBe('Thanks!');
  });

  it('appends the quote when the draft has grown shorter than its place', () => {
    const taken = takeDraftQuote(review(), 'q2')!;
    const shorter = emptyComposerDraft();

    const restored = restoreDraftQuote(shorter, taken.removed);

    expect(restored.quotes.map((q) => q.id)).toEqual(['q2']);
    expect(restored.quotes[0].before).toBe('');
    expect(restored.text).toBe('');
  });

  it('is null for an unknown id', () => {
    expect(takeDraftQuote(review(), 'missing')).toBeNull();
  });
});

describe('setDraftQuoteReply', () => {
  it('replaces one quote’s reply and nothing else', () => {
    const draft = review();

    const next = setDraftQuoteReply(draft, 'q1', 'Batched, please.');

    expect(next.quotes.map((q) => q.reply)).toEqual(['Batched, please.', 'Which flag system?']);
    expect(next.quotes[0]).toMatchObject({ before: 'A few comments:', quote: dataFlow.quote });
    expect(next.text).toBe('Thanks!');
    expect(draft.quotes[0].reply).toBe('Could we batch those?');
  });

  it('returns the draft itself for an unknown id or an unchanged reply', () => {
    const draft = review();

    expect(setDraftQuoteReply(draft, 'removed', 'anything')).toBe(draft);
    expect(setDraftQuoteReply(draft, 'q2', 'Which flag system?')).toBe(draft);
  });
});

describe('summarizeComposerDraft', () => {
  it('is null for an empty draft', () => {
    expect(summarizeComposerDraft(emptyComposerDraft())).toBeNull();
    expect(summarizeComposerDraft({ quotes: [], text: ' \n' })).toBeNull();
  });

  it('counts quotes and tells text from none', () => {
    expect(summarizeComposerDraft({ quotes: [], text: 'hi' })).toEqual({
      quotes: 0,
      hasText: true,
    });
    const quoteOnly = appendDraftQuote(emptyComposerDraft(), {
      quote: 'x',
      headings: [],
      reply: '',
    });
    expect(summarizeComposerDraft(quoteOnly)).toEqual({ quotes: 1, hasText: false });
    expect(
      summarizeComposerDraft(setDraftQuoteReply(quoteOnly, quoteOnly.quotes[0].id, 'why'))
    ).toEqual({ quotes: 1, hasText: true });
    expect(summarizeComposerDraft(review())).toEqual({ quotes: 2, hasText: true });
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
  it('writes each quote as a blockquote under its marker line, in draft order', () => {
    expect(serializeComposerDraft(review())).toBe(
      [
        'A few comments:',
        '<!-- note-quote: Architecture › Data flow -->\n> The cache is invalidated on every write.',
        'Could we batch those?',
        'And one more.',
        '<!-- note-quote: Rollout -->\n> Ship behind a flag.',
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

    expect(serializeComposerDraft(draft)).toBe(
      '<!-- note-quote -->\n> First line\n>\n> Second paragraph'
    );
  });

  it('is the trimmed text for a draft without quotes', () => {
    expect(serializeComposerDraft({ quotes: [], text: '  hello  ' })).toBe('hello');
  });

  it('sends a draft without quotes exactly as typed, open fence and all', () => {
    expect(serializeComposerDraft({ quotes: [], text: 'Look:\n```js\nfoo()' })).toBe(
      'Look:\n```js\nfoo()'
    );
  });

  it('escapes a path separator, a comment end or an escaping backslash in a heading', () => {
    const draft = appendDraftQuote(
      emptyComposerDraft(),
      { quote: 'x', headings: ['Before › after', 'a --> b', 'C:\\temp\\›'], reply: '' },
      'q1'
    );

    expect(serializeComposerDraft(draft)).toBe(
      '<!-- note-quote: Before \\› after › a --\\> b › C:\\temp\\\\\\› -->\n> x'
    );
  });

  it('puts a heading that spans lines on one', () => {
    const draft = appendDraftQuote(
      emptyComposerDraft(),
      { quote: 'x', headings: [' Two\nlines ', '  '], reply: '' },
      'q1'
    );

    expect(serializeComposerDraft(draft)).toBe('<!-- note-quote: Two lines -->\n> x');
  });

  it('closes a code fence left open before a quote or in a reply', () => {
    let draft = appendDraftQuote(
      { quotes: [], text: 'Try:\n````ts\nrun()' },
      { quote: 'One', headings: [], reply: '~~~\nstill open' },
      'q1'
    );
    draft = { ...draft, text: 'Last:\n```' };

    expect(serializeComposerDraft(draft)).toBe(
      [
        'Try:\n````ts\nrun()\n````',
        '<!-- note-quote -->\n> One',
        '~~~\nstill open\n~~~',
        // The main input ends the message, so it goes as typed.
        'Last:\n```',
      ].join('\n\n')
    );
  });

  it('leaves closed fences and inline triple backticks alone', () => {
    const before = 'Run ```npm i``` then:\n```sh\nnpm test\n```';
    const draft = appendDraftQuote(
      { quotes: [], text: before },
      { quote: 'q', headings: [], reply: '' },
      'q1'
    );

    expect(serializeComposerDraft(draft)).toBe(`${before}\n\n<!-- note-quote -->\n> q`);
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
  function roundTrip(draft: ComposerDraft) {
    return parseQuotedMessage(serializeComposerDraft(draft));
  }

  function quoteDraft(quote: Partial<NewDraftQuote>, before = '', text = ''): ComposerDraft {
    const draft = appendDraftQuote(
      { quotes: [], text: before },
      { quote: 'Quoted.', headings: [], reply: '', ...quote },
      'q1'
    );
    return { ...draft, text };
  }

  it('reads back what serializeComposerDraft wrote', () => {
    const parts = roundTrip(review());

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
    expect(roundTrip(quoteDraft({ quote: 'one\n\ntwo', reply: 'reply' }))).toEqual([
      { type: 'quote', quote: 'one\n\ntwo', headings: [] },
      { type: 'text', text: 'reply' },
    ]);
  });

  it('keeps quoted lines that start with > or spaces as they were', () => {
    expect(roundTrip(quoteDraft({ quote: '> nested\n    indented' }))).toEqual([
      { type: 'quote', quote: '> nested\n    indented', headings: [] },
    ]);
  });

  it('leaves a plain message as one text part', () => {
    const parts = parseQuotedMessage('hello\nworld');

    expect(parts).toEqual([{ type: 'text', text: 'hello\nworld' }]);
    expect(hasQuotedParts(parts)).toBe(false);
  });

  it('leaves quotes typed or pasted by hand as text', () => {
    for (const message of [
      '> one\n>\n> two\n\nreply',
      'Re: Q3 plan\n> We should ship in March.\n\nAgreed.',
      'Re: something\n\n> quoted',
      '>>> print(1)\n>not a quote',
      'Re: A\n>\n>',
    ]) {
      expect(parseQuotedMessage(message)).toEqual([{ type: 'text', text: message }]);
    }
  });

  it('reads a reply or gap that starts with > as text', () => {
    const draft = appendDraftQuote(
      { quotes: [], text: '> my own aside' },
      { quote: 'Quoted.', headings: ['A'], reply: '> not from the note\nmy answer' },
      'q1'
    );

    expect(roundTrip(draft)).toEqual([
      { type: 'text', text: '> my own aside' },
      { type: 'quote', quote: 'Quoted.', headings: ['A'] },
      { type: 'text', text: '> not from the note\nmy answer' },
    ]);
  });

  it('reads back headings holding the separator, a comment end or backslashes', () => {
    const headings = ['Before › after', 'a --> b', 'C:\\temp\\›', 'trailing \\'];

    expect(roundTrip(quoteDraft({ headings }))).toEqual([
      { type: 'quote', quote: 'Quoted.', headings },
    ]);
  });

  it('needs the marker, its > lines and a blank line or the edge around them', () => {
    for (const message of [
      // No > line under the marker.
      '<!-- note-quote: A -->\n\n> quoted',
      // Text straight above the marker.
      'intro\n<!-- note-quote -->\n> quoted',
      // Text straight below the quote.
      '<!-- note-quote -->\n> quoted\nlazy line',
      // Nothing but bare > lines.
      '<!-- note-quote -->\n>\n>',
      // Not the marker.
      '<!-- note-quote:A -->\n> quoted',
      '<!--note-quote-->\n> quoted',
    ]) {
      expect(parseQuotedMessage(message)).toEqual([{ type: 'text', text: message }]);
    }
  });

  it('does not read a marker inside fenced code as a quote', () => {
    const message = ['````md', '```', '', '<!-- note-quote -->', '> inside a fence', '````'].join(
      '\n'
    );

    expect(parseQuotedMessage(message)).toEqual([{ type: 'text', text: message }]);
  });

  it('does not take a fence line with an info string as its close', () => {
    const message = ['```', '```js', '', '<!-- note-quote -->', '> still code', '```'].join('\n');

    expect(hasQuotedParts(parseQuotedMessage(message))).toBe(false);
  });

  it('closes a fence only with its own character', () => {
    const message = ['~~~', '```', '', '<!-- note-quote -->', '> still code', '~~~'].join('\n');

    expect(hasQuotedParts(parseQuotedMessage(message))).toBe(false);
  });

  it('does not open a fence on a line of inline code', () => {
    expect(roundTrip(quoteDraft({}, 'Run ```npm i``` first'))).toEqual([
      { type: 'text', text: 'Run ```npm i``` first' },
      { type: 'quote', quote: 'Quoted.', headings: [] },
    ]);
  });

  it('reads every quote after a fence the user left open', () => {
    let draft = appendDraftQuote(
      { quotes: [], text: 'Try:\n```ts\nrun()' },
      { quote: 'One', headings: ['A'], reply: '```\nopen in the reply' },
      'q1'
    );
    draft = appendDraftQuote(draft, { quote: 'Two', headings: ['B'], reply: '' }, 'q2');

    expect(roundTrip(draft)).toEqual([
      { type: 'text', text: 'Try:\n```ts\nrun()\n```' },
      { type: 'quote', quote: 'One', headings: ['A'] },
      { type: 'text', text: '```\nopen in the reply\n```' },
      { type: 'quote', quote: 'Two', headings: ['B'] },
    ]);
  });
});
