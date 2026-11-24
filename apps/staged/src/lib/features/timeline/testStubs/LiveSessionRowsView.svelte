<!--
  A view over the session stream the way ProjectSection and BranchTimeline
  are: it gives `createLiveSessionRows` its sessions from an effect, and each
  row reads its own session's title and subtitle.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { SessionStreamOwner } from '../../../stores/sessionStream.svelte';
  import { createLiveSessionRows } from '../liveSessionHints';
  import LiveSessionRowsViewRow from './LiveSessionRowsViewRow.svelte';

  let {
    owner,
    sessionIds,
    visible,
    roots,
    onRender,
  }: {
    owner: SessionStreamOwner;
    sessionIds: string[];
    visible: boolean;
    roots: string[];
    onRender: (sessionId: string) => void;
  } = $props();

  const rows = createLiveSessionRows({
    // svelte-ignore state_referenced_locally
    owner,
    visible: () => visible,
    displayRoots: () => roots,
  });

  $effect(() => {
    rows.sync(sessionIds);
  });

  onDestroy(() => rows.destroy());
</script>

<ul>
  {#each sessionIds as sessionId (sessionId)}
    <LiveSessionRowsViewRow {rows} {sessionId} {onRender} />
  {/each}
</ul>
