export type ArtifactKind = 'commit' | 'note' | 'review';

const artifactNoun: Record<ArtifactKind, string> = {
  commit: 'commit',
  note: 'note',
  review: 'comments',
};

export function failedArtifactSubtitle(
  completionReason: string | null | undefined,
  kind: ArtifactKind
): string {
  const noun = artifactNoun[kind];
  switch (completionReason) {
    case 'crashed':
      return `Session crashed — no ${noun} created`;
    case 'app_quit':
      return `Session interrupted — no ${noun} created`;
    case 'project_session_interrupted':
      return `Session stopped by project session — no ${noun} created`;
    case 'interrupted':
      return `Session stopped — no ${noun} created`;
    default:
      return `Session finished — no ${noun} created`;
  }
}
