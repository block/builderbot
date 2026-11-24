import { beforeAll, describe, expect, it } from 'vitest';
import launchResumePlanSource from '../../../../src-tauri/src/launch_resume/plan.rs?raw';
import rateLimitSchedulerSource from '../../../../src-tauri/src/rate_limit_scheduler.rs?raw';
import { RESUME_PROMPT } from '../../shared/resumePrompt';
import { RESUME_ROW_PROMPT } from './sessionRowState';
import { STAGED_RESUME_PROMPTS, isStagedResumePrompt } from './stagedResumePrompts';

// The backend persists a resume prompt as a plain user row, and the live hint
// keeps it out of a session's subtitle by matching its text
// (`isStagedResumePrompt`). Nothing at runtime connects the two: the prompts
// are string constants in Rust and string constants here. These tests read
// the Rust sources as text, so rewording a prompt on one side, or adding a
// resume prompt the frontend has not heard of, fails here instead of putting
// Staged's own words in a subtitle.

/**
 * Every `const …RESUME_PROMPT: &str = "…";` in `source`, by name, with the
 * string's `\`-newline continuations joined the way rustc joins them. No
 * other escape is decoded: a value that still holds a `\` is not the string
 * rustc builds. It is returned as it stands, and a test below rejects it.
 */
function rustResumePrompts(source: string): Record<string, string> {
  const prompts: Record<string, string> = {};
  const declaration = /const (\w*RESUME_PROMPT): &str =\s*"((?:[^"\\]|\\[\s\S])*)";/g;
  for (const [, name, literal] of source.matchAll(declaration)) {
    prompts[name] = literal.replace(/\\\r?\n\s*/g, '');
  }
  return prompts;
}

describe('STAGED_RESUME_PROMPTS', () => {
  // Parsed in a hook, with every check on the result in a test of its own, so
  // that a Rust source this file cannot read fails a test that names what is
  // wrong, not the collection of the whole file.
  let backend: Record<string, string>;

  beforeAll(() => {
    backend = {
      ...rustResumePrompts(rateLimitSchedulerSource),
      ...rustResumePrompts(launchResumePlanSource),
    };
  });

  it('finds the resume prompts in the Rust sources', () => {
    expect(Object.keys(backend).sort()).toEqual([
      'AUTO_RETRY_RESUME_PROMPT',
      'NETWORK_RESUME_PROMPT',
      'PROJECT_SESSION_RESUME_PROMPT',
      'REPO_SESSION_RESUME_PROMPT',
    ]);
  });

  it('reads each Rust literal without an escape it cannot decode', () => {
    // Any escape other than a line continuation would need decoding before
    // the word-for-word comparison below means anything.
    const undecoded = Object.keys(backend).filter((name) => backend[name].includes('\\'));
    expect(undecoded).toEqual([]);
  });

  it('matches the backend word for word, with nothing missing on either side', () => {
    expect({ ...STAGED_RESUME_PROMPTS }).toEqual(backend);
  });

  it('uses the Resume buttons’ prompt for the automatic retry', () => {
    expect(backend.AUTO_RETRY_RESUME_PROMPT).toBe(RESUME_PROMPT);
  });

  it('gives a row’s Resume button the same prompt as the chat’s', () => {
    expect(RESUME_ROW_PROMPT).toBe(RESUME_PROMPT);
  });
});

describe('isStagedResumePrompt', () => {
  it('recognises each resume prompt', () => {
    for (const prompt of Object.values(STAGED_RESUME_PROMPTS)) {
      expect(isStagedResumePrompt(prompt)).toBe(true);
    }
  });

  it('ignores surrounding and repeated whitespace', () => {
    expect(isStagedResumePrompt('  Continue where\nyou left off.\n')).toBe(true);
  });

  it('does not match a reply that only starts like one', () => {
    expect(isStagedResumePrompt('Continue where you left off, but skip the tests')).toBe(false);
    expect(isStagedResumePrompt('Continue')).toBe(false);
  });
});
