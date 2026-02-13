/**
 * Reactive state for the right-column detail panel.
 *
 * When a commit or note is clicked in the timeline, the panel opens
 * alongside the branch list in ProjectHome. Only one item is shown
 * at a time — opening a new one replaces the previous.
 */

export type DetailPanelState =
  | { kind: 'none' }
  | {
      kind: 'note';
      branchId: string;
      noteId: string;
      title: string;
      content: string;
      sessionId: string | null;
    }
  | {
      kind: 'commit';
      branchId: string;
      commitSha: string;
      shortSha: string;
      subject: string;
      sessionId: string | null;
    };

export const detailPanel = $state<{ current: DetailPanelState }>({
  current: { kind: 'none' },
});

export function openNote(
  branchId: string,
  noteId: string,
  title: string,
  content: string,
  sessionId: string | null
): void {
  detailPanel.current = { kind: 'note', branchId, noteId, title, content, sessionId };
}

export function openCommit(
  branchId: string,
  commitSha: string,
  shortSha: string,
  subject: string,
  sessionId: string | null
): void {
  detailPanel.current = { kind: 'commit', branchId, commitSha, shortSha, subject, sessionId };
}

export function closeDetail(): void {
  detailPanel.current = { kind: 'none' };
}
