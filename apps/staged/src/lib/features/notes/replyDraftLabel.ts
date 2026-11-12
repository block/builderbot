/**
 * What the note view's bottom bar calls the chat's unsent reply draft (see
 * replyDrafts.replyDraftPresence): the quotes it holds, when it holds any.
 */

import type { ComposerDraftSummary } from '../sessions/composerDraft';

export function formatReplyDraftLabel(summary: ComposerDraftSummary | null): string | null {
  if (!summary) return null;
  if (summary.quotes === 0) return 'Unsent reply draft';
  return `Unsent reply draft · ${summary.quotes} ${summary.quotes === 1 ? 'quote' : 'quotes'}`;
}
