/**
 * Which projects a note can move or copy to, and the words the picker uses for
 * each. NoteTransferDialog's list, its "create project" row and its buttons all
 * read from here, so the rules have exactly one frontend statement.
 */
import { projectSearchName } from '../branches/moveBranchTarget';
import type { Project } from '../../types';

export type NoteTransferMode = 'move' | 'copy';
export type NoteTransferKind = 'branch' | 'project';

export interface NoteTransferLabels {
  title: string;
  confirm: string;
  busy: string;
}

export function noteTransferLabels(mode: NoteTransferMode): NoteTransferLabels {
  return mode === 'move'
    ? { title: 'Move to project', confirm: 'Move', busy: 'Moving...' }
    : { title: 'Copy to project', confirm: 'Copy', busy: 'Copying...' };
}

/**
 * The projects offered as destinations. A move out of the current project
 * can't land back in it (the backend rejects that), so it is left off the
 * list; a copy into the current project is a duplicate, which is allowed.
 * When the note brings child notes along (`childrenTravel`), remote projects
 * are left off too: a remote project's branches share one workspace, so the
 * children's branches could not be re-homed there and the backend refuses.
 */
export function noteTransferCandidates(
  projects: Project[],
  currentProjectId: string | null | undefined,
  mode: NoteTransferMode,
  options: { localOnly?: boolean } = {}
): Project[] {
  const eligible = options.localOnly ? projects.filter((p) => p.location !== 'remote') : projects;
  if (mode === 'copy' || !currentProjectId) return eligible;
  return eligible.filter((p) => p.id !== currentProjectId);
}

/**
 * Whether the transfer carries child notes: only a *moved* project note that
 * has some. A copy is standalone, and a branch note has no children.
 */
export function childrenTravel(
  mode: NoteTransferMode,
  noteKind: NoteTransferKind,
  hasChildren: boolean
): boolean {
  return mode === 'move' && noteKind === 'project' && hasChildren;
}

/**
 * The name a "Create project" row would use for `query`, or `null` when the
 * row should not be shown: nothing typed, or a project already goes by that
 * name (case-insensitively) — picking the existing one beats creating a twin.
 * "Goes by" is `projectSearchName`, the same name the list is filtered on.
 */
export function createProjectOption(query: string, projects: Project[]): string | null {
  const name = query.trim();
  if (!name) return null;
  const lower = name.toLowerCase();
  if (projects.some((p) => projectSearchName(p) === lower)) return null;
  return name;
}

/** The repo + subpath a branch note is bound to. */
export interface NoteRepo {
  githubRepo: string;
  subpath: string | null;
}

/**
 * The picker's lookup of the stored note's repo, made each time it opens.
 * `repo` is `null` for a note bound to none: every project note, and a branch
 * note whose branch tracks no repo.
 */
export type NoteRepoLookup =
  { status: 'loading' } | { status: 'resolved'; repo: NoteRepo | null } | { status: 'failed' };

/** The looked-up repo, or `null` while there isn't one to act on. */
export function lookedUpRepo(lookup: NoteRepoLookup): NoteRepo | null {
  return lookup.status === 'resolved' ? lookup.repo : null;
}

/**
 * Why a project created for this note can't be remote, or `null` when it can.
 * A new remote project starts from the note's repo, which provisions its
 * workspace, so that repo has to be known first. A note bound to no repo —
 * every project note — makes a remote project with no repo, whose workspace
 * waits for the first repo added to it.
 */
export function newRemoteProjectBlocker(lookup: NoteRepoLookup): string | null {
  switch (lookup.status) {
    case 'loading':
      return "Checking this note's repo for Remote…";
    case 'failed':
      return "Remote is unavailable: couldn't look up this note's repo";
    case 'resolved':
      return null;
  }
}

/**
 * Why the stored note can't be moved or copied right now, or `null` when it
 * can. This is the backend's own refusal (`ensure_settled` in
 * note_transfer.rs: a session still writing the note) plus a note that no
 * longer exists, checked before the picker creates a project for the note so
 * a refused transfer leaves no project behind. The backend checks again.
 */
export function noteTransferBlocker(
  note: { title: string; sessionId: string | null; completedAt: number | null } | null,
  mode: NoteTransferMode
): string | null {
  if (!note) return 'This note no longer exists.';
  if (note.sessionId && note.completedAt === null) {
    const name = note.title.trim() ? `"${note.title}"` : 'This note';
    const gerund = mode === 'move' ? 'moving' : 'copying';
    return `${name} is still being generated; wait for its session to finish before ${gerund} it.`;
  }
  return null;
}

/**
 * Why a branch note can't be moved or copied for want of a repo, or `null`
 * when `repo` (the stored note's, `null` for none) is there. The transfer adds
 * the note's repo to the destination, so the backend refuses a note whose
 * branch is bound to none (`transfer_branch_note` in note_transfer.rs); this
 * is its wording, short of the branch name the picker doesn't have. Checked
 * before the picker creates a project, like `noteTransferBlocker`.
 */
export function unboundBranchNoteBlocker(
  repo: NoteRepo | null,
  mode: NoteTransferMode
): string | null {
  if (repo) return null;
  const participle = mode === 'move' ? 'moved' : 'copied';
  return `This note's branch is not bound to a repository, so the note cannot be ${participle} to another project.`;
}

/**
 * The inline error for a transfer that failed after the picker created its
 * destination. The project stays (and stays picked, so a retry goes there),
 * so the error says it exists rather than reading as if nothing happened.
 */
export function transferFailedAfterCreate(
  mode: NoteTransferMode,
  projectName: string,
  reason: string
): string {
  return `Created ${projectName}, but the note wasn't ${mode === 'move' ? 'moved' : 'copied'}: ${reason}`;
}

/** What the picker has settled on: an existing project or a name to create. */
export type NoteTransferSelection =
  { kind: 'existing'; projectId: string } | { kind: 'new'; name: string };

/**
 * What confirming (Move/Copy, or Enter in the search box) acts on: the picked
 * row or, when nothing is picked, the "Create project" row if it is the only
 * row on offer, else the single match left by what was typed. With nothing
 * typed nothing is assumed, even when only one project is on offer, so Enter
 * straight after opening can't send the note somewhere the user never chose.
 * `null` when that leaves nothing to act on, which keeps the confirm button
 * disabled.
 */
export function noteTransferTarget(
  selected: NoteTransferSelection | null,
  query: string,
  filtered: Project[],
  createName: string | null
): NoteTransferSelection | null {
  if (selected) return selected;
  if (!query.trim()) return null;
  if (filtered.length === 1 && !createName) return { kind: 'existing', projectId: filtered[0].id };
  if (filtered.length === 0 && createName) return { kind: 'new', name: createName };
  return null;
}

/**
 * Explains, for a repo note, what the destination will gain. Project notes
 * carry no repo, so there is nothing to say.
 */
export function repoAddedHint(repo: NoteRepo | null | undefined): string | null {
  if (!repo) return null;
  const path = repo.subpath ? `${repo.githubRepo} (${repo.subpath})` : repo.githubRepo;
  return `${path} is added to the project if it isn't there yet.`;
}

/**
 * What travels with the note, shown under its title in the picker. A repo
 * note brings its repo (see `repoAddedHint`). A moved project note that has
 * child notes brings them, sessions included, and whatever repos they are
 * bound to; the commits and code reviews its session produced have no link
 * back to it and stay on their branch. A project note without children, or a
 * copy (which is standalone), moves alone, so there is nothing to say.
 */
export function noteTransferHint(
  mode: NoteTransferMode,
  noteKind: NoteTransferKind,
  repo: { githubRepo: string; subpath: string | null } | null | undefined,
  hasChildren = false
): string | null {
  if (noteKind === 'branch') return repoAddedHint(repo);
  if (!childrenTravel(mode, noteKind, hasChildren)) return null;
  return (
    'moves with its child notes and their sessions; any repository they need is added to ' +
    'the project. Commits and code reviews stay on their branch.'
  );
}
