/**
 * The prompt a Resume button sends to the session it resumes: the two in the
 * session chat and the one on a timeline row (`RESUME_ROW_PROMPT`).
 *
 * The exact words matter. The backend sends the same sentence when it resumes
 * a session by itself (`AUTO_RETRY_RESUME_PROMPT` in
 * `rate_limit_scheduler.rs`), and the live hint tells a resume from a reply
 * the user typed by matching this text (`stagedResumePrompts.ts`). A button
 * that sent its own copy of the sentence could be reworded without any test
 * noticing, and would then show in the session's subtitle as a reply.
 */
export const RESUME_PROMPT = 'Continue where you left off.';
