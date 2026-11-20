import { describe, expect, it } from 'vitest';
import remoteScriptSource from '../../../../src-tauri/resources/remote/staged-mcp.cjs?raw';
import noteToolSource from '../../../../src-tauri/src/note_tool.rs?raw';
import sessionCommandsSource from '../../../../src-tauri/src/session_commands.rs?raw';
import { NOTE_DONE_WORD, isNoteDoneText } from './noteDoneMessage';

// The backend asks a note session to close its turn with one quoted word, and
// the chat pane hides a message that is nothing but that word. Nothing at
// runtime connects the two: the request is text in Rust and in the script
// uploaded to Blox, and the filter is `isNoteDoneText` here. These tests read
// the backend sources as text, so rewording either side without the other
// fails here instead of leaving a visible closing word in every note chat.

/**
 * The phrase that carries the closing word, in each source's own spelling:
 * `"Done"` in a single-quoted JavaScript string, `\"Done\"` in a Rust or
 * double-quoted JavaScript one.
 */
const CLOSING_REQUEST = /send a simple \\?"([^"\\]*)\\?" message/g;

/**
 * Source text with its string literals joined back up, so a phrase wrapped
 * across lines still reads as one: Rust's `\` line continuations and
 * JavaScript's `"a " + "b"` joins.
 */
function joinStringLiterals(source: string): string {
  return source.replace(/\\\r?\n[ \t]*/g, '').replace(/(["'])\s*\+\s*(["'])/g, '');
}

/** The text from `start` up to the first `end` after it. */
function sourceBetween(source: string, start: string, end: string): string {
  const from = source.indexOf(start);
  expect(from, `"${start}" was not found; update this test with the rename`).toBeGreaterThan(-1);
  const to = source.indexOf(end, from + start.length);
  expect(to, `no "${end}" after "${start}"`).toBeGreaterThan(from);
  return joinStringLiterals(source.slice(from, to));
}

/** Every word a text asks the agent to close its turn with. */
function closingWords(text: string): string[] {
  return [...text.matchAll(CLOSING_REQUEST)].map((match) => match[1]);
}

const resultTexts: ReadonlyArray<readonly [label: string, text: string]> = [
  [
    'the local tool result (note_tool.rs saved_message)',
    sourceBetween(noteToolSource, 'pub(crate) fn saved_message(title: &str', '\n}\n'),
  ],
  [
    'the Blox tool result (staged-mcp.cjs savedMessage)',
    sourceBetween(remoteScriptSource, 'function savedMessage(title', '\n}\n'),
  ],
];

const guidanceTexts: ReadonlyArray<readonly [label: string, text: string]> = [
  [
    'the local tool description (note_tool.rs #[tool] attribute)',
    sourceBetween(noteToolSource, '#[tool(\n        description = ', '\n    )]'),
  ],
  [
    'the shared tool description (note_tool.rs WRITE_NOTE_DESCRIPTION)',
    sourceBetween(noteToolSource, 'pub(crate) const WRITE_NOTE_DESCRIPTION', ';\n'),
  ],
  [
    'the Blox tool description (staged-mcp.cjs WRITE_NOTE_DESCRIPTION)',
    sourceBetween(remoteScriptSource, 'const WRITE_NOTE_DESCRIPTION =', ';\n'),
  ],
  [
    'the session prompt (session_commands.rs note_delivery_guidance)',
    sourceBetween(sessionCommandsSource, 'pub(crate) fn note_delivery_guidance(', '\n}\n'),
  ],
];

describe('the closing word the backend asks for', () => {
  it.each(resultTexts)('%s quotes the one word the filter hides', (_label, text) => {
    expect(closingWords(text)).toEqual([NOTE_DONE_WORD]);
    expect(isNoteDoneText(closingWords(text)[0])).toBe(true);
  });

  it.each(guidanceTexts)('%s asks for the same word', (_label, text) => {
    expect(closingWords(text)).toEqual([NOTE_DONE_WORD]);
    expect(isNoteDoneText(closingWords(text)[0])).toBe(true);
  });

  it('gives local and Blox sessions the same instruction after the saved sentence', () => {
    // Everything from the byte count on is fixed text; what comes before it
    // is built from the title and length, in each language's own syntax.
    const instructions = resultTexts.map(([, text]) => {
      const match = /bytes from the draft file\)\. (.*?)\\?["']\s*\)/s.exec(text);
      expect(match, text).not.toBeNull();
      return match![1].replace(/\\"/g, '"');
    });
    expect(instructions[0]).toBe(
      'If the user asked you a question this turn and you have not answered it in chat yet, ' +
        `answer it now, briefly. Otherwise send a simple "${NOTE_DONE_WORD}" message with no ` +
        'other text to end your turn, rather than summarising the work done.'
    );
    expect(instructions[1]).toBe(instructions[0]);
  });
});

describe('reading the request out of source text', () => {
  it('finds the word in each spelling of the quotes', () => {
    expect(closingWords('send a simple "Done" message')).toEqual(['Done']);
    expect(closingWords('send a simple \\"Done\\" message')).toEqual(['Done']);
    expect(closingWords('send a simple "Finished" message')).toEqual(['Finished']);
    expect(closingWords('end your turn without repeating the note')).toEqual([]);
  });

  it('reads a phrase wrapped across lines as one', () => {
    expect(joinStringLiterals('send a \\\n        simple \\"Done\\" message')).toBe(
      'send a simple \\"Done\\" message'
    );
    expect(joinStringLiterals('"send a simple " +\n  \'"Done" message\'')).toBe(
      '"send a simple "Done" message\''
    );
  });
});
