/**
 * Which projects a note can move or copy to, and the words the picker uses for
 * each. NoteTransferDialog's list, its "create project" row and its buttons all
 * read from here, so the rules have exactly one frontend statement.
 */
import { projectSearchName } from '../branches/moveBranchTarget';
import type { Project } from '../../types';

export type NoteTransferMode = 'move' | 'copy';

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
 */
export function noteTransferCandidates(
  projects: Project[],
  currentProjectId: string | null | undefined,
  mode: NoteTransferMode
): Project[] {
  if (mode === 'copy' || !currentProjectId) return projects;
  return projects.filter((p) => p.id !== currentProjectId);
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

/**
 * Why a project created for this note can't be remote, or `null` when it can.
 * A remote project's workspace is provisioned from the repo it is created
 * with, and adding a repo afterwards needs that workspace running already, so
 * a new remote project starts from the note's repo — and project notes have
 * none.
 */
export function newRemoteProjectBlocker(
  repo: { githubRepo: string; subpath: string | null } | null | undefined
): string | null {
  return repo ? null : 'Needs a repo; this note has none';
}

/** What the picker has settled on: an existing project or a name to create. */
export type NoteTransferSelection =
  { kind: 'existing'; projectId: string } | { kind: 'new'; name: string };

/**
 * Explains, for a repo note, what the destination will gain. Project notes
 * carry no repo, so there is nothing to say.
 */
export function repoAddedHint(
  repo: { githubRepo: string; subpath: string | null } | null | undefined
): string | null {
  if (!repo) return null;
  const path = repo.subpath ? `${repo.githubRepo} (${repo.subpath})` : repo.githubRepo;
  return `${path} is added to the project if it isn't there yet.`;
}
