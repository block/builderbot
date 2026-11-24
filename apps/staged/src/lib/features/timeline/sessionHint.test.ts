import { describe, expect, it } from 'vitest';
import type { MessageRole, SessionMessage } from '../../types';
import { deriveHint, type HintRow } from './sessionHint';
import { STAGED_RESUME_PROMPTS } from './stagedResumePrompts';

function row(id: number, role: MessageRole, content: string, imageIds?: string[]): SessionMessage {
  return { id, sessionId: 's1', role, content, createdAt: id, ...(imageIds ? { imageIds } : {}) };
}

const OPENING_PROMPT = row(
  1,
  'user',
  '<action>\nThe user is requesting a note.\n</action>\n\nSummarise the cache layer'
);
const TURN_END = row(2, 'assistant', 'Done');
const REPLY = row(3, 'user', 'Also cover the eviction policy');

type Fixture = {
  name: string;
  /** Whole rows in id order. The first `user` row is the opening prompt. */
  transcript: SessionMessage[];
  displayRoots?: string[];
  hint: string | undefined;
};

/**
 * The opening prompt as the poller (`liveSessionHints.ts`) and the backend
 * (`opening_prompt_id` in `session_summary.rs`) both find it: the first
 * `user` row.
 */
function openingPromptId(transcript: readonly HintRow[]): number | null {
  return transcript.find((message) => message.role === 'user')?.id ?? null;
}

function hintOf(rows: readonly HintRow[], { transcript, displayRoots = [] }: Fixture) {
  return deriveHint(rows, displayRoots, openingPromptId(transcript));
}

const FIXTURES: Fixture[] = [
  {
    name: 'shows nothing for an empty transcript',
    transcript: [],
    hint: undefined,
  },
  {
    name: 'shows nothing for a session that only has its opening prompt',
    transcript: [OPENING_PROMPT],
    hint: undefined,
  },
  {
    name: 'shows a tool call',
    transcript: [OPENING_PROMPT, row(2, 'tool_call', 'Read')],
    hint: 'Reading…',
  },
  {
    name: 'shows a tool call’s path relative to a display root',
    transcript: [
      OPENING_PROMPT,
      row(
        2,
        'tool_call',
        JSON.stringify({ name: 'Read', arguments: { file_path: '/repo/src/cache.ts' } })
      ),
    ],
    displayRoots: ['/repo'],
    hint: 'Reading src/cache.ts…',
  },
  {
    name: 'stays on a tool call once its result is in',
    transcript: [
      OPENING_PROMPT,
      row(2, 'tool_call', 'Read'),
      row(3, 'tool_result', 'export const ttl = 300;'),
    ],
    hint: 'Reading…',
  },
  {
    name: 'shows nothing when a tool result is all that follows the opening prompt',
    transcript: [OPENING_PROMPT, row(2, 'tool_result', 'export const ttl = 300;')],
    hint: undefined,
  },
  {
    name: 'shows the last line of assistant text, without its list marker',
    transcript: [
      OPENING_PROMPT,
      row(2, 'assistant', 'Looking at the cache layer.\n\n- Reading the eviction code'),
    ],
    hint: 'Reading the eviction code',
  },
  {
    name: 'skips assistant text that carries a review block',
    transcript: [
      OPENING_PROMPT,
      row(2, 'assistant', 'Checking the diff'),
      row(3, 'assistant', 'I now have a complete picture.\n\n```review-title\nCache review\n```'),
    ],
    hint: 'Checking the diff',
  },
  {
    name: 'shows a reply sent after a completed turn in place of that turn’s last message',
    transcript: [OPENING_PROMPT, TURN_END, REPLY],
    hint: 'Also cover the eviction policy',
  },
  {
    name: 'lets a newer tool call replace the reply',
    transcript: [OPENING_PROMPT, TURN_END, REPLY, row(4, 'tool_call', 'Read')],
    hint: 'Reading…',
  },
  {
    name: 'lets newer assistant text replace the reply',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      REPLY,
      row(4, 'tool_call', 'Read'),
      row(5, 'assistant', 'Looking at the eviction code'),
    ],
    hint: 'Looking at the eviction code',
  },
  {
    name: 'shows the latest of several replies',
    transcript: [OPENING_PROMPT, TURN_END, REPLY, row(4, 'user', 'And keep it short')],
    hint: 'And keep it short',
  },
  {
    name: 'shows the text the user typed, not the blocks Staged wrapped around it',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      row(
        3,
        'user',
        '<action>\nThe user is asking you to update the linked note.\n</action>\n\n' +
          '<branch-history>\nEarlier sessions…\n</branch-history>\n\n' +
          'Tighten   the intro\nand drop the last section'
      ),
    ],
    hint: 'Tighten the intro and drop the last section',
  },
  {
    name: 'leaves the quoted passage out of a reply to a note selection',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      row(
        3,
        'user',
        '<!-- note-quote: Overview › Caching -->\n> Entries expire after five minutes.\n\n' +
          'Is that configurable?'
      ),
    ],
    hint: 'Is that configurable?',
  },
  {
    // A `write_note` reminder is a user row that is all action block: there is
    // nothing the user typed in it, so the agent's last message stays.
    name: 'skips a message Staged wrote on the user’s behalf',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', '<action>\nYour turn ended without a `write_note` call.\n</action>'),
    ],
    hint: 'Done',
  },
  // The backend stores a resume prompt as a plain user row, with no action
  // block around it: nobody typed it, so it must not read as a reply.
  ...Object.entries(STAGED_RESUME_PROMPTS).flatMap(([name, prompt]): Fixture[] => [
    {
      name: `the resume prompt ${name} leaves the agent’s last message showing`,
      transcript: [OPENING_PROMPT, TURN_END, row(3, 'user', prompt)],
      hint: 'Done',
    },
    {
      name: `the resume prompt ${name} leaves an earlier reply showing`,
      transcript: [OPENING_PROMPT, TURN_END, REPLY, row(4, 'user', prompt)],
      hint: 'Also cover the eviction policy',
    },
    {
      // Skipped only because the tagged blocks are stripped before the text
      // is matched against the resume prompts; this pins that order.
      name: `the resume prompt ${name} is skipped behind a block Staged wrapped around it`,
      transcript: [
        OPENING_PROMPT,
        TURN_END,
        row(3, 'user', `<action>\nThe user is requesting a note.\n</action>\n\n${prompt}`),
      ],
      hint: 'Done',
    },
    {
      name: `the resume prompt ${name} gives way to what the agent writes after the resume`,
      transcript: [
        OPENING_PROMPT,
        TURN_END,
        row(3, 'user', prompt),
        row(4, 'assistant', 'Picking up the eviction section'),
      ],
      hint: 'Picking up the eviction section',
    },
  ]),
  {
    name: 'shows a reply that goes on from a resume prompt’s words',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', 'Continue where you left off. Skip the benchmarks this time'),
    ],
    hint: 'Continue where you left off. Skip the benchmarks this time',
  },
  {
    // Stored as a plain user row, like a typed reply: it is an instruction to
    // the session and the transcript shows it as a user bubble.
    name: 'shows a message a project agent sent with continue_repo_session',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', 'Rebase onto main and re-run the cache tests'),
    ],
    hint: 'Rebase onto main and re-run the cache tests',
  },
  {
    name: 'shows the content of a reply that is only a fenced block',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', '```ts\nconst ttl = 300;\ncache.set(key, value, ttl);\n```'),
    ],
    hint: 'const ttl = 300; cache.set(key, value, ttl);',
  },
  {
    name: 'keeps a fenced block’s content in line with the text around it',
    transcript: [
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', 'Use this instead:\n\n~~~\nttl = 300\n~~~\n\nand keep `ttl` an int'),
    ],
    hint: 'Use this instead: ttl = 300 and keep `ttl` an int',
  },
  ...['use Vec<String> here', 'if a < b and c > d'].map((typed): Fixture => ({
    name: `keeps angle brackets in typed text: ${typed}`,
    transcript: [OPENING_PROMPT, TURN_END, row(3, 'user', typed)],
    hint: typed,
  })),
  {
    name: 'shows a placeholder for a reply that is an attachment and no text',
    transcript: [OPENING_PROMPT, TURN_END, row(3, 'user', '', ['img-1'])],
    hint: 'Attachment',
  },
  {
    name: 'counts the attachments of a reply with several and no text',
    transcript: [OPENING_PROMPT, TURN_END, row(3, 'user', '  \n', ['img-1', 'img-2'])],
    hint: '2 attachments',
  },
  {
    name: 'shows the typed text of a reply that also has an attachment',
    transcript: [OPENING_PROMPT, TURN_END, row(3, 'user', 'Match this mock-up', ['img-1'])],
    hint: 'Match this mock-up',
  },
  {
    // The agent's echo of a delivered message is stored this way, and so is a
    // row whose attachment list came back empty.
    name: 'skips an empty user row with no attachment',
    transcript: [OPENING_PROMPT, TURN_END, row(3, 'user', ''), row(4, 'user', '', [])],
    hint: 'Done',
  },
];

describe('deriveHint', () => {
  it.each(FIXTURES)('$name', (fixture) => {
    expect(hintOf(fixture.transcript, fixture)).toBe(fixture.hint);
  });

  it('truncates a long reply', () => {
    const hint = deriveHint([OPENING_PROMPT, TURN_END, row(3, 'user', 'word '.repeat(40))], [], 1);

    expect(hint).toHaveLength(72);
    expect(hint?.endsWith('…')).toBe(true);
  });

  it('drops an emoji the cut would split instead of leaving half of it', () => {
    // An emoji outside the BMP is two UTF-16 units. Seventy letters put the
    // cut at unit 71 between the halves of the first one.
    const reply = row(3, 'user', `${'a'.repeat(70)}😀😀`);

    expect(deriveHint([OPENING_PROMPT, TURN_END, reply], [], 1)).toBe(`${'a'.repeat(70)}…`);
  });

  it('keeps an emoji that ends where the cut falls', () => {
    const reply = row(3, 'user', `${'a'.repeat(69)}😀😀`);

    expect(deriveHint([OPENING_PROMPT, TURN_END, reply], [], 1)).toBe(`${'a'.repeat(69)}😀…`);
  });

  it('does not split an emoji when it truncates agent text either', () => {
    const text = row(2, 'assistant', `${'a'.repeat(70)}😀😀`);

    expect(deriveHint([OPENING_PROMPT, text], [], 1)).toBe(`${'a'.repeat(70)}…`);
  });
});

// The session stream sends a summary row, not a whole message: the backend's
// `SummaryRow` (`src-tauri/src/store/session_summary.rs`) is the row's id,
// role and attachment ids, and its text unless the row is a `tool_result` or
// the opening prompt. Leaving those two texts out is only safe while
// `deriveHint` never reads them. If it starts to, the two derivations below
// part ways and the backend has to send the text again.
describe('deriveHint over the rows a session summary carries', () => {
  /** `message` as the backend's `summary_rows` query projects it. */
  function summaryRow(message: SessionMessage, openingId: number | null): HintRow {
    const omitsContent = message.role === 'tool_result' || message.id === openingId;
    return {
      id: message.id,
      role: message.role,
      ...(omitsContent ? {} : { content: message.content }),
      ...(message.imageIds?.length ? { imageIds: message.imageIds } : {}),
    };
  }

  function summaryRows(transcript: SessionMessage[]): HintRow[] {
    const openingId = openingPromptId(transcript);
    return transcript.map((message) => summaryRow(message, openingId));
  }

  it.each(FIXTURES)('gives the same hint as whole messages: $name', (fixture) => {
    expect(hintOf(summaryRows(fixture.transcript), fixture)).toBe(
      hintOf(fixture.transcript, fixture)
    );
  });

  it('is exercised by fixtures that have both of the texts a summary leaves out', () => {
    const omitted = FIXTURES.flatMap(({ transcript }) => {
      const rows = summaryRows(transcript);
      return transcript.filter((message, i) => message.content && rows[i].content === undefined);
    });

    expect(omitted.some((message) => message.role === 'tool_result')).toBe(true);
    expect(omitted.some((message) => message.id === OPENING_PROMPT.id)).toBe(true);
  });
});
