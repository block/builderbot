import { describe, expect, it } from 'vitest';
import {
  foldSnippetsIntoPrompt,
  formatToolDisplay,
  insertedText,
  isAwaitingRemoteNote,
  isStoringRemoteNote,
  makePathsRelative,
  newestUserMessageId,
  reconnectedOutcomeMessage,
  REMOTE_NOTE_POLL_SUFFIX,
  REMOTE_NOTE_UNACKNOWLEDGED_STOP_MARKER,
  REMOTE_NOTE_UNREAD_OUTBOX_MARKER,
  isUnacknowledgedRemoteStop,
  isUnreadRemoteOutbox,
  REMOTE_NOTE_STORING_SUFFIX,
  sessionEndMessage,
  sessionErrorMessage,
  sessionEndNoticeKind,
  snippetLabel,
} from './sessionModalHelpers';
import {
  RECONCILE_MAY_STILL_RUN,
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_WORKSTATION_GONE,
} from '../../shared/disconnectedSession';

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

describe('insertedText', () => {
  it('returns text appended to the end', () => {
    expect(insertedText('commit text', 'commit text typed')).toBe(' typed');
  });

  it('returns text inserted in the middle', () => {
    expect(insertedText('abc', 'abXYc')).toBe('XY');
  });

  it('returns everything when starting from an empty prompt', () => {
    expect(insertedText('', 'typed')).toBe('typed');
  });

  it('returns the replacement when a selection is overwritten', () => {
    expect(insertedText('hello world', 'hello there world')).toBe('there ');
  });

  it("returns '' for unchanged text and pure deletions", () => {
    expect(insertedText('same', 'same')).toBe('');
    expect(insertedText('commit text', 'commit')).toBe('');
    expect(insertedText('commit', '')).toBe('');
  });

  it('drops boundary characters a replacement shares with the replaced text (known limitation)', () => {
    // A prefix/suffix diff can't tell a shared boundary character apart from
    // untouched text; accepted for the one-round-trip window it covers.
    expect(insertedText('note text', 'new')).toBe('ew');
    expect(insertedText('fix bug', 'fix bag')).toBe('a');
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

  it('explains a truncated background wait', () => {
    expect(sessionEndMessage({ completionReason: 'held_until_cap' })).toBe(
      'Staged stopped waiting for this session’s background work.'
    );
  });

  it('explains a stopped background wait', () => {
    expect(sessionEndMessage({ completionReason: 'hold_stopped' })).toBe(
      'The wait for this session’s background work was stopped.'
    );
  });

  it('explains a lost workspace agent', () => {
    expect(sessionEndMessage({ completionReason: 'remote_agent_lost' })).toBe(
      'This session’s agent stopped running on its workspace.'
    );
  });

  it('tells a detached session still being checked from one Staged gave up on', () => {
    const polling = `Staged lost this session while the agent was running on ws-1; ${REMOTE_NOTE_POLL_SUFFIX}`;
    const expired =
      'Staged lost this session while the agent was running on ws-1, and no completed note appeared on the workstation within 30 minutes. Resume to check again and continue.';
    expect(isAwaitingRemoteNote(polling)).toBe(true);
    expect(isAwaitingRemoteNote(expired)).toBe(false);
    expect(isAwaitingRemoteNote(null)).toBe(false);
    expect(sessionEndMessage({ completionReason: 'detached', errorMessage: polling })).toBe(
      'Staged lost the connection to this remote session. The workstation agent may still be ' +
        'running; Staged is checking for its note and will complete the session when it appears.'
    );
    // The backend's other detached messages are written for the user and
    // already say what Resume does: shown as they are.
    expect(sessionEndMessage({ completionReason: 'detached', errorMessage: expired })).toBe(
      expired
    );
    expect(sessionEndMessage({ completionReason: 'detached' })).toBe(
      'Staged lost this remote session before it delivered its result.'
    );
  });

  it('warns that a stop the workstation didn’t confirm may not have taken', () => {
    expect(
      sessionEndMessage({ completionReason: 'interrupted', errorMessage: RECONCILE_MAY_STILL_RUN })
    ).toBe(
      'You stopped this session. The workstation didn’t confirm the stop, so the turn may still be running there.'
    );
  });

  it('says what reconcile found when it couldn’t settle the turn', () => {
    expect(
      sessionEndMessage({
        completionReason: 'interrupted',
        errorMessage: RECONCILE_TURN_UNDETERMINED,
      })
    ).toBe('Staged couldn’t tell whether this turn finished on the workstation.');
    expect(
      sessionEndMessage({ completionReason: 'interrupted', errorMessage: RECONCILE_NO_WORKSPACE })
    ).toBe('Staged couldn’t find this session’s workstation.');
  });

  it('says the newest message never reached the agent', () => {
    expect(
      sessionEndMessage({
        completionReason: 'interrupted',
        errorMessage: RECONCILE_PROMPT_NOT_RECEIVED,
      })
    ).toBe(
      'Staged lost its connection before your last message reached the workstation, so the agent never saw it.'
    );
  });

  it('ignores messages it doesn’t recognise', () => {
    expect(
      sessionEndMessage({ completionReason: 'interrupted', errorMessage: 'something else' })
    ).toBe('You stopped this session.');
  });
});

describe('sessionErrorMessage', () => {
  it('reframes a deleted workstation around the lost turn', () => {
    expect(sessionErrorMessage(RECONCILE_WORKSTATION_GONE)).toBe(
      'This session’s workstation no longer exists, so Staged couldn’t catch up with its last turn.'
    );
  });

  it('renders any other error as written', () => {
    expect(sessionErrorMessage('Blox process exited with code 1')).toBe(
      'Blox process exited with code 1'
    );
  });
});

describe('reconnectedOutcomeMessage', () => {
  it('says Staged caught up when a reconciled turn completed', () => {
    expect(reconnectedOutcomeMessage({ status: 'completed' })).toBe(
      'Staged reconnected to the workstation and caught up. The agent finished this turn.'
    );
  });

  it('leaves other outcomes to their own notices', () => {
    for (const status of ['error', 'cancelled', 'running', 'disconnected'] as const) {
      expect(reconnectedOutcomeMessage({ status })).toBeNull();
    }
  });
});

describe('newestUserMessageId', () => {
  it('finds the newest user message, past the replies after it', () => {
    expect(
      newestUserMessageId([
        { id: 1, role: 'user' },
        { id: 2, role: 'assistant' },
        { id: 5, role: 'user' },
        { id: 6, role: 'assistant' },
        { id: 7, role: 'tool_call' },
      ])
    ).toBe(5);
  });

  it('is null when no user message is loaded', () => {
    expect(newestUserMessageId([])).toBeNull();
    expect(newestUserMessageId([{ id: 3, role: 'assistant' }])).toBeNull();
  });
  it('explains API limit failures', () => {
    expect(sessionEndMessage({ completionReason: 'rate_limited' })).toBe(
      'This session paused because the provider reported an API limit.'
    );
  });

  it('explains API limit reset time when available', () => {
    expect(
      sessionEndMessage({
        completionReason: 'rate_limited',
        rateLimitResetsAt: '2026-06-24T15:45:00Z',
      })
    ).toContain('It resets around');
  });

  it('explains scheduled API limit auto-retry state', () => {
    expect(
      sessionEndMessage({
        completionReason: 'rate_limited',
        providerMetadata: {
          staged: {
            rateLimitAutoRecovery: {
              attempt: 3,
              maxAttempts: 8,
              nextRetryAt: Date.UTC(2026, 5, 24, 15, 45),
            },
          },
        },
      })
    ).toContain('It will retry around');
  });

  it('explains API limit auto-retry give-up state', () => {
    expect(
      sessionEndMessage({
        completionReason: 'rate_limited',
        providerMetadata: {
          staged: {
            rateLimitAutoRecovery: {
              attempt: 9,
              maxAttempts: 8,
              gaveUpAt: Date.UTC(2026, 5, 24, 15, 45),
            },
          },
        },
      })
    ).toBe(
      'This session paused because the provider reported an API limit. Auto-retry gave up after 9 attempts.'
    );
  });

  it('explains API limit manual-only recovery state', () => {
    expect(
      sessionEndMessage({
        completionReason: 'rate_limited',
        providerMetadata: {
          staged: {
            rateLimitAutoRecovery: {
              unsafeToAutoResume: true,
              reason: 'pending_tool_call',
            },
          },
        },
      })
    ).toBe(
      'This session paused because the provider reported an API limit. Resume manually when ready.'
    );
  });

  it('keeps the API limit copy for an API-limit kind', () => {
    expect(
      sessionEndMessage({ completionReason: 'rate_limited', rateLimitKind: 'five_hour' })
    ).toBe('This session paused because the provider reported an API limit.');
  });

  it('explains a network pause', () => {
    expect(sessionEndMessage({ completionReason: 'rate_limited', rateLimitKind: 'network' })).toBe(
      'Paused: network unavailable. Resumes when the connection returns.'
    );
  });

  it('gives no retry time for a network pause waiting behind the gate', () => {
    // As the scheduler writes it: `nextRetryAt` is when the row paused, so
    // it is already in the past and stays there for the whole outage.
    const pausedAt = Date.UTC(2026, 5, 24, 15, 45);
    expect(
      sessionEndMessage({
        completionReason: 'rate_limited',
        rateLimitKind: 'network',
        providerMetadata: {
          staged: {
            rateLimitAutoRecovery: {
              attempt: 0,
              maxAttempts: 8,
              nextRetryAt: pausedAt,
              reason: 'network_wait',
            },
          },
        },
      })
    ).toBe('Paused: network unavailable. Resumes when the connection returns.');
  });

  it('says when a network pause on its own backoff tries again', () => {
    const nextRetryAt = Date.UTC(2026, 5, 24, 15, 45);
    const retryTime = new Date(nextRetryAt).toLocaleTimeString([], {
      hour: 'numeric',
      minute: '2-digit',
    });
    expect(
      sessionEndMessage({
        completionReason: 'rate_limited',
        rateLimitKind: 'network',
        providerMetadata: {
          staged: {
            rateLimitAutoRecovery: { attempt: 2, maxAttempts: 8, nextRetryAt, reason: 'backoff' },
          },
        },
      })
    ).toBe(
      `Paused: network unavailable. Resumes when the connection returns. Trying again around ${retryTime}.`
    );
  });

  it('asks for a manual resume of a network pause that will not resume by itself', () => {
    for (const rateLimitAutoRecovery of [
      { attempt: 9, maxAttempts: 8, gaveUpAt: Date.UTC(2026, 5, 24, 15, 45) },
      { attempt: 0, gaveUpAt: Date.UTC(2026, 5, 24, 15, 45), reason: 'probe_limit_exceeded' },
      { unsafeToAutoResume: true, reason: 'pending_tool_call' },
    ]) {
      expect(
        sessionEndMessage({
          completionReason: 'rate_limited',
          rateLimitKind: 'network',
          providerMetadata: { staged: { rateLimitAutoRecovery } },
        })
      ).toBe('Paused: network unavailable. Resume manually when ready.');
    }
  });
});

describe('detached remote note sessions', () => {
  it('does not promise a note after a Stop the remote agent never acknowledged', () => {
    const stopped = `Staged lost the connection to this session while the agent was running on ws-1 (the agent ${REMOTE_NOTE_UNACKNOWLEDGED_STOP_MARKER} within 10 seconds; the stop may still have reached it, so a note may or may not appear); ${REMOTE_NOTE_POLL_SUFFIX}`;
    expect(isUnacknowledgedRemoteStop(stopped)).toBe(true);
    expect(isAwaitingRemoteNote(stopped)).toBe(true);
    expect(isUnacknowledgedRemoteStop(null)).toBe(false);
    expect(sessionEndMessage({ completionReason: 'detached', errorMessage: stopped })).toBe(
      'You stopped this remote session, but the workstation did not confirm it in time. ' +
        'The stop may still have reached the agent, so a note may or may not appear; ' +
        'Staged is checking the workstation and will complete the session if one does.'
    );
    // Once the sweep gives up the backend's own copy is shown, Stop or not.
    const expired =
      'Staged lost this session while the agent was running on ws-1, and no completed note appeared on the workstation within 30 minutes. Resume to check again and continue.';
    expect(sessionEndMessage({ completionReason: 'detached', errorMessage: expired })).toBe(
      expired
    );
  });

  it('says the turn ended when only the note outbox could not be read', () => {
    const unread = `This session's turn ended on ws-1, but Staged ${REMOTE_NOTE_UNREAD_OUTBOX_MARKER} (failed to read the remote note outbox: ssh: connect to host ws-1: Operation timed out); ${REMOTE_NOTE_POLL_SUFFIX}`;
    expect(isUnreadRemoteOutbox(unread)).toBe(true);
    expect(isAwaitingRemoteNote(unread)).toBe(true);
    expect(isUnreadRemoteOutbox(null)).toBe(false);
    const message = sessionEndMessage({ completionReason: 'detached', errorMessage: unread });
    expect(message).toBe(
      'The turn of this remote session ended, but Staged could not read its note from the ' +
        'workstation. Staged keeps checking and will complete the session when it finds the ' +
        'note.'
    );
    expect(message).not.toContain('lost the connection');
    expect(message).not.toContain('may still be running');
    // A row that lost its connection keeps the lost-connection copy.
    const lost = `Staged lost the connection to this session while the agent was running on ws-1 (incoming transport closed); ${REMOTE_NOTE_POLL_SUFFIX}`;
    expect(isUnreadRemoteOutbox(lost)).toBe(false);
    expect(sessionEndMessage({ completionReason: 'detached', errorMessage: lost })).toContain(
      'Staged lost the connection to this remote session'
    );
  });

  it('tells a detached session whose note is being stored', () => {
    const storing = `Staged lost this session while the agent was running on ws-1; ${REMOTE_NOTE_STORING_SUFFIX}`;
    expect(isStoringRemoteNote(storing)).toBe(true);
    expect(isAwaitingRemoteNote(storing)).toBe(false);
    expect(isStoringRemoteNote(null)).toBe(false);
    expect(sessionEndMessage({ completionReason: 'detached', errorMessage: storing })).toBe(
      'Staged lost the connection to this remote session, but the workstation agent ' +
        'finished its note. Staged is storing it and will complete the session.'
    );
  });
});

describe('sessionEndNoticeKind', () => {
  const polling = `Staged lost the connection to this session while the agent was running on ws-1 (incoming transport closed); ${REMOTE_NOTE_POLL_SUFFIX}`;

  it('shows a detached row with an error message as the resumable detached notice, not a raw error', () => {
    // Every detached row the backend writes carries an error_message; the
    // raw-error branch would hide the Resume action and the polling copy.
    const detached = {
      status: 'error',
      completionReason: 'detached',
      errorMessage: polling,
    } as const;
    expect(sessionEndNoticeKind(detached)).toBe('detached');
    expect(sessionEndMessage(detached)).toContain('Staged is checking for its note');
    expect(
      sessionEndNoticeKind({
        status: 'error',
        completionReason: 'detached',
        errorMessage:
          'Staged lost this session while the agent was running on ws-1; its note "Plan" is complete on the workstation but could not be stored here. Resume to collect it again.',
      })
    ).toBe('detached');
  });

  it('keeps the raw error for other failed runs and the generic notice for resumable ones', () => {
    expect(
      sessionEndNoticeKind({
        status: 'error',
        completionReason: 'crashed',
        errorMessage: 'ACP authentication is required to run the prompt.',
      })
    ).toBe('error');
    expect(
      sessionEndNoticeKind({
        status: 'cancelled',
        completionReason: 'interrupted',
        errorMessage: 'killed',
      })
    ).toBe('error');
    expect(sessionEndNoticeKind({ status: 'error', completionReason: 'crashed' })).toBe(
      'resumable'
    );
    expect(sessionEndNoticeKind({ status: 'completed', completionReason: 'turn_complete' })).toBe(
      null
    );
    expect(sessionEndNoticeKind({ status: 'running', completionReason: null })).toBe(null);
    expect(
      sessionEndNoticeKind({
        status: 'running',
        completionReason: 'detached',
        errorMessage: polling,
      })
    ).toBe(null);
  });
});
