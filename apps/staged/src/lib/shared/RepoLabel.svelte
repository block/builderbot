<!--
  RepoLabel – reusable repo-path display with contrast styling.

  The *last* path segment gets full contrast (--text-primary) while every
  preceding segment is muted (--text-muted).  This makes the most
  distinguishing part of the path pop out visually.

  Examples:
    githubRepo="block/mark"              → "block/" muted  + "mark" primary
    githubRepo="block/mark" subpath="ui" → "block/mark/" muted + "ui" primary
-->
<script lang="ts">
  interface Props {
    githubRepo: string;
    subpath?: string | null;
  }

  let { githubRepo, subpath = null }: Props = $props();

  let prefix = $derived.by(() => {
    if (subpath) {
      return githubRepo + '/';
    }
    const idx = githubRepo.lastIndexOf('/');
    return idx >= 0 ? githubRepo.slice(0, idx + 1) : '';
  });

  let emphasis = $derived.by(() => {
    if (subpath) {
      return subpath;
    }
    const idx = githubRepo.lastIndexOf('/');
    return idx >= 0 ? githubRepo.slice(idx + 1) : githubRepo;
  });

  let fullLabel = $derived(subpath ? `${githubRepo}/${subpath}` : githubRepo);
</script>

<span class="repo-label" title={fullLabel}
  >{#if prefix}<span class="repo-label-prefix">{prefix}</span>{/if}<span class="repo-label-emphasis"
    >{emphasis}</span
  ></span
>

<style>
  .repo-label {
    display: inline;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-label-prefix {
    color: var(--text-muted);
  }

  .repo-label-emphasis {
    color: var(--text-primary);
  }
</style>
