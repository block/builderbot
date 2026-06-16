import { describe, expect, it } from 'vitest';
import {
  CLIPBOARD_SNIPPET_MIN_LENGTH,
  foldSnippetsIntoPrompt,
  formatToolDisplay,
  makePathsRelative,
  sessionEndMessage,
  shouldOfferClipboardSnippet,
  snippetLabel,
} from './sessionModalHelpers';

describe('foldSnippetsIntoPrompt', () => {
  it('returns the prompt unchanged when there are no snippets', () => {
    expect(foldSnippetsIntoPrompt('do the thing', [])).toBe('do the thing');
  });

  it('appends each snippet wrapped in <attached-snippet> delimiters', () => {
    expect(
      foldSnippetsIntoPrompt('do the thing', [{ text: 'context one' }, { text: 'context two' }])
    ).toBe(
      'do the thing' +
        '\n\n<attached-snippet>\ncontext one\n</attached-snippet>' +
        '\n\n<attached-snippet>\ncontext two\n</attached-snippet>'
    );
  });

  it('folds snippets onto an empty prompt (snippet-only submit)', () => {
    expect(foldSnippetsIntoPrompt('', [{ text: 'just context' }])).toBe(
      '\n\n<attached-snippet>\njust context\n</attached-snippet>'
    );
  });
});

describe('shouldOfferClipboardSnippet', () => {
  it('is false for empty or nullish clipboard text', () => {
    expect(shouldOfferClipboardSnippet(null)).toBe(false);
    expect(shouldOfferClipboardSnippet(undefined)).toBe(false);
    expect(shouldOfferClipboardSnippet('')).toBe(false);
  });

  it('gates strictly above the threshold length', () => {
    const atThreshold = 'a'.repeat(CLIPBOARD_SNIPPET_MIN_LENGTH);
    const overThreshold = 'a'.repeat(CLIPBOARD_SNIPPET_MIN_LENGTH + 1);
    expect(shouldOfferClipboardSnippet(atThreshold)).toBe(false);
    expect(shouldOfferClipboardSnippet(overThreshold)).toBe(true);
  });
});

describe('snippetLabel', () => {
  it('collapses whitespace to a single line', () => {
    expect(snippetLabel('first line\n  second line')).toBe('first line second line');
  });

  it('truncates long previews with an ellipsis', () => {
    expect(snippetLabel('x'.repeat(100), 10)).toBe('xxxxxxxxx…');
  });

  it('falls back to a generic label for whitespace-only text', () => {
    expect(snippetLabel('   \n  ')).toBe('snippet');
  });
});

describe('makePathsRelative', () => {
  it('matches symlink-style alias pairs', () => {
    expect(
      makePathsRelative('Read /Volumes/work/staged/repo/src/App.svelte', [
        '/Users/me/.staged/repo',
        '/Volumes/work/staged/repo',
      ])
    ).toBe('Read src/App.svelte');
  });

  it('uses the longest matching root before broader project roots', () => {
    expect(
      makePathsRelative(
        '/work/projects/p1/builderbot--feature/src/App.svelte /work/projects/p1/other--main/README.md',
        ['/work/projects/p1', '/work/projects/p1/builderbot--feature']
      )
    ).toBe('src/App.svelte other--main/README.md');
  });

  it('applies the ancestor fallback for subpath working directories', () => {
    expect(makePathsRelative('/work/repo/src/lib.rs', '/work/repo/apps/staged')).toBe('src/lib.rs');
  });

  it('applies the ancestor fallback after direct matches', () => {
    expect(
      makePathsRelative(
        'cat /work/repo/apps/staged/src/App.svelte /work/repo/src-tauri/src/lib.rs',
        '/work/repo/apps/staged'
      )
    ).toBe('cat src/App.svelte src-tauri/src/lib.rs');
  });

  it('formats shell command strings with resolved alias roots', () => {
    const call = JSON.stringify({
      name: 'Bash',
      arguments: {
        command:
          'sed -n "1,5p" /Volumes/work/repo/src/main.ts && cat /Users/me/link/repo/package.json',
      },
    });

    expect(formatToolDisplay(call, ['/Users/me/link/repo', '/Volumes/work/repo'], true)).toEqual({
      verb: 'Running',
      detail: 'sed -n "1,5p" src/main.ts && cat package.json',
    });
  });
});

describe('formatToolDisplay', () => {
  it('uses parsed command query for search tool calls', () => {
    const call = JSON.stringify({
      name: 'Search fn running_branch_session_kinds in session_commands.rs',
      input: {
        parsed_cmd: [
          {
            type: 'search',
            cmd: "rg -n 'fn running_branch_session_kinds' src-tauri/src/session_commands.rs",
            query: 'fn running_branch_session_kinds',
            path: 'session_commands.rs',
          },
        ],
      },
    });

    expect(formatToolDisplay(call, '/repo')).toEqual({
      verb: 'Searched',
      detail: 'fn running_branch_session_kinds',
    });
  });

  it('uses parsed command paths for read and write tool calls', () => {
    const readCall = JSON.stringify({
      name: 'Read writer.rs',
      input: {
        parsed_cmd: [
          {
            type: 'read',
            cmd: 'sed -n "1,80p" /repo/src-tauri/src/agent/writer.rs',
            name: 'writer.rs',
            path: '/repo/src-tauri/src/agent/writer.rs',
          },
        ],
      },
    });
    const writeCall = JSON.stringify({
      name: 'Write sessionModalHelpers.ts',
      input: {
        parsed_cmd: [
          {
            type: 'write',
            cmd: 'apply_patch src/lib/features/sessions/sessionModalHelpers.ts',
            name: 'sessionModalHelpers.ts',
            path: '/repo/src/lib/features/sessions/sessionModalHelpers.ts',
          },
        ],
      },
    });

    expect(formatToolDisplay(readCall, '/repo')).toEqual({
      verb: 'Read',
      detail: 'src-tauri/src/agent/writer.rs',
    });
    expect(formatToolDisplay(writeCall, '/repo')).toEqual({
      verb: 'Wrote',
      detail: 'src/lib/features/sessions/sessionModalHelpers.ts',
    });
  });

  it('keeps top-level search args ahead of parsed command fallbacks', () => {
    const call = JSON.stringify({
      name: 'Search',
      input: {
        query: 'top level query',
        parsed_cmd: [{ type: 'search', query: 'parsed query' }],
      },
    });

    expect(formatToolDisplay(call)).toEqual({
      verb: 'Searched',
      detail: 'top level query',
    });
  });
});

describe('sessionEndMessage', () => {
  it('explains project-session interruptions', () => {
    expect(sessionEndMessage({ completionReason: 'project_session_interrupted' })).toBe(
      'This session was stopped by its project session.'
    );
  });

  it('explains direct interruptions as user stops', () => {
    expect(sessionEndMessage({ completionReason: 'interrupted' })).toBe(
      'You stopped this session.'
    );
  });
});
