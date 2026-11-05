<!--
  RemoteProjectIcon.svelte — the muted cloud that marks a remote project.

  Location is kept apart from status: the project's status icon (see
  ProjectStatusIcon) reads the same whether it is local or remote, and this
  sits on the subtitle line beneath it. Shared by the sidebar rows
  (ProjectRowContent) and the project grid cards; parents place it.
-->
<script lang="ts">
  import Cloud from '@lucide/svelte/icons/cloud';

  interface Props {
    size?: number;
  }

  let { size = 12 }: Props = $props();
</script>

<span class="remote-project-icon" role="img" aria-label="Remote project" title="Remote project">
  <Cloud {size} />
</span>

<style>
  .remote-project-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-faint);
  }
</style>
