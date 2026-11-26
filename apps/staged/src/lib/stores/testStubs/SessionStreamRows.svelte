<!-- A list of session rows over one session stream owner. -->
<script lang="ts">
  import type { DisplayRootInput } from '../../features/sessions/pathDisplayRoots';
  import type { SessionStreamOwner } from '../sessionStream.svelte';
  import SessionStreamRow from './SessionStreamRow.svelte';

  let {
    owner,
    sessionIds,
    roots,
    onRender,
  }: {
    owner: SessionStreamOwner;
    sessionIds: string[];
    roots: DisplayRootInput;
    onRender: (sessionId: string) => void;
  } = $props();
</script>

<ul>
  {#each sessionIds as sessionId (sessionId)}
    <SessionStreamRow {owner} {sessionId} {roots} {onRender} />
  {/each}
</ul>
