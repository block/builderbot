// The data a run needs: git repositories, projects, a branch with a worktree
// per session. All through the app's own commands, into the throwaway HOME.

import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { log, until } from './util.mjs';

const IDENTITY = [
  '-c',
  'user.name=Perf Harness',
  '-c',
  'user.email=perf-harness@example.invalid',
  '-c',
  'init.defaultBranch=main',
  '-c',
  'commit.gpgsign=false',
];

/**
 * A repository the app treats as already cloned: an origin inside the run
 * directory, and a clone of it where the app keeps `local/<name>`. Nothing
 * here has a remote on the network.
 */
function seedRepository({ runDir, home, name, env }) {
  const git = (cwd, ...args) =>
    execFileSync('/usr/bin/git', [...IDENTITY, ...args], {
      cwd,
      env,
      stdio: ['ignore', 'ignore', 'pipe'],
    });
  const origin = join(runDir, 'origins', `${name}.git`);
  const work = join(runDir, 'origins', `${name}-seed`);
  mkdirSync(join(work, 'src'), { recursive: true });
  writeFileSync(join(work, 'README.md'), `# ${name}\n\nSeeded by the Staged load-test harness.\n`);
  writeFileSync(join(work, 'src', 'main.ts'), `export const name = '${name}';\n`);
  git(work, 'init', '--quiet');
  git(work, 'add', '.');
  git(work, 'commit', '--quiet', '-m', 'init');
  git(runDir, 'clone', '--quiet', '--bare', work, origin);
  const clone = join(home, '.staged', 'repos', 'local', name);
  mkdirSync(join(home, '.staged', 'repos', 'local'), { recursive: true });
  git(runDir, 'clone', '--quiet', origin, clone);
}

/** Returns `[{ id, name, branches: [{ id, name }] }]`, one branch per session. */
export async function seed({ api, runDir, home, env, projects, sessionsPerProject }) {
  const out = [];
  for (let p = 0; p < projects; p++) {
    const repo = `perf-${p}`;
    const name = `Perf project ${p}`;
    seedRepository({ runDir, home, name: repo, env });
    const project = await api.invoke('create_project', {
      name,
      githubRepo: `local/${repo}`,
      defaultBranch: 'main',
    });
    const branches = () => api.invoke('list_branches_for_project', { projectId: project.id });
    for (let b = (await branches()).length; b < sessionsPerProject; b++) {
      await api.invoke('create_branch', {
        projectId: project.id,
        branchName: `load-${p}-${b}`,
        baseBranch: 'main',
      });
    }
    out.push({ id: project.id, name, branches: [] });
  }
  // Worktrees last and together: each is a git checkout the app queues.
  for (const project of out) {
    const listed = (await api.invoke('list_branches_for_project', { projectId: project.id })).slice(
      0,
      sessionsPerProject
    );
    for (const branch of listed) {
      await until(
        `the worktree of ${project.name} / ${branch.branchName}`,
        async () => {
          await api.invoke('setup_worktree', { branchId: branch.id }).catch(() => null);
          const now = (
            await api.invoke('list_branches_for_project', { projectId: project.id })
          ).find((b) => b.id === branch.id);
          return now?.worktreePath ? now : null;
        },
        { timeout: 180_000, every: 2000 }
      );
      project.branches.push({ id: branch.id, name: branch.branchName });
    }
    log(`seeded ${project.name}: ${project.branches.length} branches`);
  }
  return out;
}
