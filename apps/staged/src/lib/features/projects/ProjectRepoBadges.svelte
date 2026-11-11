<!--
  ProjectRepoBadges.svelte — a project card's repos, sorted, each as its badge
  (the repo's hue) or a plain label; a project whose repos haven't loaded
  falls back to its own repo, and one with none says so. Shared by the home
  grid's cards and the board's.
-->
<script lang="ts">
  import type { Project, ProjectRepo } from '../../types';
  import { darkMode } from '../../stores/isDark.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { badgeBg, badgeFg } from '../../shared/badgeColors';
  import RepoLabel from '../../shared/RepoLabel.svelte';

  interface Props {
    project: Project;
    repos: ProjectRepo[];
  }

  let { project, repos }: Props = $props();

  function repoKey(repo: Pick<ProjectRepo, 'githubRepo' | 'subpath'>): string {
    return repo.subpath ? `${repo.githubRepo}/${repo.subpath}` : repo.githubRepo;
  }

  let labels = $derived(
    repos.length > 0
      ? [...repos]
          .sort((a, b) => repoKey(a).localeCompare(repoKey(b)))
          .map((repo) => ({
            githubRepo: repo.githubRepo,
            shownRepo: repo.headRepo ?? repo.githubRepo,
            subpath: repo.subpath,
          }))
      : project.githubRepo
        ? [
            {
              githubRepo: project.githubRepo,
              shownRepo: project.githubRepo,
              subpath: project.subpath,
            },
          ]
        : []
  );
</script>

<div class="repo">
  {#each labels as label, i (i)}
    {@const badge = repoBadgeStore.lookup(label.githubRepo, label.subpath)}
    {#if badge}
      <span
        class="repo-badge-label"
        style="background: {badgeBg(badge.hue, darkMode.value)}; color: {badgeFg(
          badge.hue,
          darkMode.value
        )};"
      >
        <RepoLabel githubRepo={label.shownRepo} subpath={label.subpath} />
      </span>
    {:else}
      <span class="repo-line">
        <RepoLabel githubRepo={label.shownRepo} subpath={label.subpath} />
      </span>
    {/if}
  {:else}
    No repo attached
  {/each}
</div>

<style>
  .repo {
    flex: 1;
    min-width: 0;
    font-size: var(--size-sm);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 4px;
    overflow: hidden;
  }

  .repo-line {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    gap: 5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-badge-label {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    padding: 1px 5px;
    border-radius: 4px;
    font-weight: 600;
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .repo-badge-label :global(.repo-label-prefix) {
    color: inherit;
    opacity: 0.6;
  }

  .repo-badge-label :global(.repo-label-emphasis) {
    color: inherit;
  }
</style>
