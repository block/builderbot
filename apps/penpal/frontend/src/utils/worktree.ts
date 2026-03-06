/**
 * Worktree URL encoding utilities.
 *
 * URL pattern: /project/QN@worktree or /file/QN@worktree/path
 * The @ separator splits the project qualified name from the worktree name.
 * Main worktree has no @ suffix.
 */

export function parseProjectWorktree(qnWithWorktree: string): {
  project: string;
  worktree: string;
} {
  const atIdx = qnWithWorktree.indexOf('@');
  if (atIdx === -1) {
    return { project: qnWithWorktree, worktree: '' };
  }
  return {
    project: qnWithWorktree.slice(0, atIdx),
    worktree: qnWithWorktree.slice(atIdx + 1),
  };
}

export function buildProjectWorktreeQN(project: string, worktree?: string): string {
  if (!worktree) return project;
  return `${project}@${worktree}`;
}

export function projectURL(project: string, worktree?: string): string {
  return `/project/${buildProjectWorktreeQN(project, worktree)}`;
}

export function fileURL(project: string, worktree: string | undefined, path: string): string {
  return `/file/${buildProjectWorktreeQN(project, worktree)}/${path}`;
}
