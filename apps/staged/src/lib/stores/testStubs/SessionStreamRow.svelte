<!--
  One session's row, reading its title and subtitle from the session stream's
  owner the way a row does: straight from the owner, for its own session.
  `onRender` is called from an effect that makes the same two reads as the
  markup, so it counts the times the row's markup is evaluated again. The
  reads are deliberately not behind a `$derived`, which would hide a read
  that is invalidated and comes back with the value it had.
-->
<script lang="ts">
  import type { DisplayRootInput } from '../../features/sessions/pathDisplayRoots';
  import type { SessionStreamOwner } from '../sessionStream.svelte';

  let {
    owner,
    sessionId,
    roots,
    onRender,
  }: {
    owner: SessionStreamOwner;
    sessionId: string;
    roots: DisplayRootInput;
    onRender: (sessionId: string) => void;
  } = $props();

  $effect.pre(() => {
    void [owner.title(sessionId), owner.hint(sessionId, roots)];
    onRender(sessionId);
  });
</script>

<li data-session={sessionId}>
  <span data-part="title">{owner.title(sessionId) ?? ''}</span>
  <span data-part="hint">{owner.hint(sessionId, roots) ?? ''}</span>
</li>
