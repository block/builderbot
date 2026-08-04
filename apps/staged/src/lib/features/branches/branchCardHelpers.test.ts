import { describe, expect, it } from 'vitest';
import {
  classifyCompletedPushSession,
  classifyPipelinePushCompletion,
  extractPrNumber,
  extractPrUrl,
  isGitActionInFlight,
  isImageFile,
  isMaybeTextFile,
  isPushRejectedNonFastForward,
} from './branchCardHelpers';
import type { PipelineExecution } from '../../types';

describe('extractPrUrl', () => {
  it('returns the canonical GitHub PR URL from a PR_URL marker', () => {
    expect(
      extractPrUrl([
        {
          role: 'assistant',
          content: 'PR_URL: https://github.com/block/builderbot/pull/123?expand=1',
        },
      ])
    ).toBe('https://github.com/block/builderbot/pull/123');
  });

  it('ignores placeholder PR_URL markers that do not contain a real PR number', () => {
    expect(
      extractPrUrl([
        {
          role: 'assistant',
          content: 'Return the required line exactly like this: PR_URL: https://github.com/...',
        },
      ])
    ).toBeNull();
  });

  it('strips surrounding markdown punctuation from fallback URL matches', () => {
    expect(
      extractPrUrl([
        {
          role: 'assistant',
          content: 'Created it successfully: <https://github.com/block/builderbot/pull/456>.',
        },
      ])
    ).toBe('https://github.com/block/builderbot/pull/456');
  });
});

describe('extractPrNumber', () => {
  it('reads the PR number from a canonical URL', () => {
    expect(extractPrNumber('https://github.com/block/builderbot/pull/456')).toBe(456);
  });
});

describe('file type helpers', () => {
  it('recognizes backend-supported image extensions', () => {
    for (const extension of ['png', 'jpg', 'jpeg', 'gif', 'webp']) {
      const filePath = `/tmp/image.${extension}`;

      expect(isImageFile(filePath)).toBe(true);
      expect(isMaybeTextFile(filePath)).toBe(false);
    }
  });

  it('treats unsupported image formats as maybe-text files', () => {
    for (const extension of ['svg', 'bmp', 'tiff', 'ico', 'heic', 'avif']) {
      const filePath = `/tmp/image.${extension}`;

      expect(isImageFile(filePath)).toBe(false);
      expect(isMaybeTextFile(filePath)).toBe(true);
    }
  });
});

describe('isPushRejectedNonFastForward', () => {
  it('recognizes the legacy assistant marker', () => {
    expect(
      isPushRejectedNonFastForward([
        {
          role: 'assistant',
          content: 'PUSH_REJECTED: NON_FAST_FORWARD',
        },
      ])
    ).toBe(true);
  });

  it('recognizes git non-fast-forward output from tool results', () => {
    expect(
      isPushRejectedNonFastForward([
        {
          role: 'tool_result',
          content: '! [rejected] feature -> feature (non-fast-forward)',
        },
      ])
    ).toBe(true);
  });

  it('ignores user prompt text that mentions non-fast-forward pushes', () => {
    expect(
      isPushRejectedNonFastForward([
        {
          role: 'user',
          content: 'If this push is non-fast-forward, do not force push.',
        },
      ])
    ).toBe(false);
  });
});

describe('classifyPipelinePushCompletion', () => {
  function pipeline(overrides: Partial<PipelineExecution> = {}): PipelineExecution {
    return {
      completedWithoutAi: false,
      currentStep: 0,
      steps: [],
      ...overrides,
    };
  }

  it('classifies a failed pipeline push with non-fast-forward output as rejected', () => {
    expect(
      classifyPipelinePushCompletion(
        pipeline({
          steps: [
            {
              label: 'Push',
              stepType: 'command',
              status: 'failed',
              output: '! [rejected] feature -> feature (non-fast-forward)',
              error: null,
              startedAt: 1,
              completedAt: 2,
            },
          ],
        })
      )
    ).toBe('rejected_non_fast_forward');
  });

  it('classifies a pipeline that completed without AI as succeeded', () => {
    expect(
      classifyPipelinePushCompletion(
        pipeline({
          completedWithoutAi: true,
          steps: [
            {
              label: 'Push',
              stepType: 'command',
              status: 'succeeded',
              output: 'To github.com:block/builderbot.git',
              error: null,
              startedAt: 1,
              completedAt: 2,
            },
          ],
        })
      )
    ).toBe('succeeded');
  });

  it('treats non-fast-forward as succeeded when AI handled recovery', () => {
    // A force-push with --force-with-lease can fail with output containing
    // "non-fast-forward" on some git server implementations. If AI ran after
    // the failure and the session completed, the push was handled successfully.
    expect(
      classifyPipelinePushCompletion(
        pipeline({
          steps: [
            {
              label: 'Push',
              stepType: 'command',
              status: 'failed',
              output: '! [rejected] feature -> feature (non-fast-forward)',
              error: null,
              startedAt: 1,
              completedAt: 2,
            },
          ],
        }),
        [
          {
            role: 'assistant',
            content: 'The force push failed because the remote ref moved. Retrying...',
          },
        ]
      )
    ).toBe('succeeded');
  });

  it('falls back to messages for AI handoff sessions', () => {
    expect(
      classifyCompletedPushSession(
        pipeline({
          steps: [
            {
              label: 'Push',
              stepType: 'command',
              status: 'failed',
              output: 'pre-push hook failed',
              error: 'Command failed with exit code 1',
              startedAt: 1,
              completedAt: 2,
            },
          ],
        }),
        [
          {
            role: 'assistant',
            content: 'Fixed the hook failure and pushed successfully.',
          },
        ]
      )
    ).toBe('succeeded');
  });
});

describe('isGitActionInFlight', () => {
  it('reports a push or pull that is running or waiting on the branch queue', () => {
    expect(isGitActionInFlight({ push: { state: 'pushing' } })).toBe(true);
    expect(isGitActionInFlight({ push: { state: 'queued' } })).toBe(true);
    expect(isGitActionInFlight({ pull: { state: 'pulling' } })).toBe(true);
    expect(isGitActionInFlight({ pull: { state: 'queued' } })).toBe(true);
    expect(isGitActionInFlight({ immediatePull: true })).toBe(true);
  });

  it('ignores finished push state, which no longer blocks anything', () => {
    expect(isGitActionInFlight({ push: { state: 'done' } })).toBe(false);
    expect(isGitActionInFlight({ push: { state: 'error' } })).toBe(false);
    expect(isGitActionInFlight({ push: { state: 'idle' } })).toBe(false);
  });

  it('reports an idle branch when neither store has an entry', () => {
    expect(isGitActionInFlight({})).toBe(false);
    expect(isGitActionInFlight({ push: null, pull: null, immediatePull: false })).toBe(false);
  });
});
