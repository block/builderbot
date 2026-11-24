import { RESUME_PROMPT } from '../../shared/resumePrompt';

/**
 * The prompts Staged sends to pick a session back up without the user typing
 * anything, keyed by the name of the Rust constant that holds each one. The
 * backend persists them as plain `user` rows with no tagged block around
 * them, so their text is the only thing that tells them from a reply.
 *
 * - `AUTO_RETRY_RESUME_PROMPT`, `NETWORK_RESUME_PROMPT`
 *   (`rate_limit_scheduler.rs`): the automatic resume after a rate-limit or
 *   network pause. The first is also what every Resume button sends
 *   (`RESUME_PROMPT`).
 * - `REPO_SESSION_RESUME_PROMPT`, `PROJECT_SESSION_RESUME_PROMPT`
 *   (`launch_resume/plan.rs`): the resume of a session that was running when
 *   Staged was closed.
 *
 * Nothing at runtime connects these strings to the backend's;
 * `stagedResumePrompts.test.ts` reads the Rust sources and fails when a
 * prompt is reworded or a new one is added on one side only.
 */
export const STAGED_RESUME_PROMPTS: Readonly<Record<string, string>> = {
  AUTO_RETRY_RESUME_PROMPT: RESUME_PROMPT,
  NETWORK_RESUME_PROMPT:
    'Your previous turn was cut off by a network error before it finished. Check the state of your work, then continue where you left off.',
  REPO_SESSION_RESUME_PROMPT:
    'Staged was closed and relaunched while you were working, so your previous turn was interrupted. Check the working tree (for example `git status` and `git log -1`) before continuing, then continue where you left off.',
  PROJECT_SESSION_RESUME_PROMPT:
    'Staged was closed and relaunched while you were working. The repo sessions you started were restarted where possible; call `wait_for_repo_session` again on any handle you were waiting on. A handle that reports `cancelled` or `failed` with reason `app_quit` could not be restarted and may be continued with `continue_repo_session` to finish its own goal, or started again. Then continue where you left off.',
};

const RESUME_PROMPT_TEXTS = new Set(Object.values(STAGED_RESUME_PROMPTS));

/** Whether `text`, with its whitespace collapsed, is one of Staged's resume prompts. */
export function isStagedResumePrompt(text: string): boolean {
  return RESUME_PROMPT_TEXTS.has(text.replace(/\s+/g, ' ').trim());
}
