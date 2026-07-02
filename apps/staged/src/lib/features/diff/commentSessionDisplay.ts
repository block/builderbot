import Clock from '@lucide/svelte/icons/clock';
import FileText from '@lucide/svelte/icons/file-text';
import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
import Spinner from '../../shared/Spinner.svelte';
import type { CommentSessionState } from '../../types';

export type CommentSessionKind = 'note' | 'commit';
export type CommentSessionDisplayContext = 'action' | 'badge';

export function getCommentSessionDisplay(
  kind: CommentSessionKind,
  state: CommentSessionState,
  context: CommentSessionDisplayContext
) {
  const kindLabel = kind === 'note' ? 'Note' : 'Commit';

  switch (state) {
    case 'queued':
      return {
        icon: Clock,
        title: `${kindLabel} session queued`,
      };
    case 'running':
      return {
        icon: Spinner,
        title: `${kindLabel} session in progress`,
      };
    case 'completed':
      if (kind === 'note') {
        return {
          icon: FileText,
          title: context === 'action' ? 'Open note' : 'Note ready',
        };
      }

      return {
        icon: GitCommitVertical,
        title: context === 'action' ? 'Show commit' : 'Commit ready',
      };
    case 'idle':
      return kind === 'note'
        ? {
            icon: FileText,
            title: 'New note (Option+click to skip dialog)',
          }
        : {
            icon: GitCommitVertical,
            title: 'New commit (Option+click to skip dialog)',
          };
  }
}
