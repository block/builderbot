<!--
  ComposerError.svelte — One failure line for the chat composer area.

  Shows why the last composer action failed (a refused send, a queue change,
  an attachment the pane rejected) in the composer's own danger style, as a
  live region so it is announced when it appears. Rendered by ChatComposer
  for the attachment and send errors, and by SessionChatPane in the
  pipeline-stop area that replaces the composer while a workflow runs with
  no transcript yet, so a failed Stop is visible there too.

  Renders nothing when `message` is null. The parent clears the message when
  the next action starts, so a repeated identical failure remounts the line
  and is announced again.
-->
<script lang="ts">
  interface Props {
    message: string | null;
  }

  let { message }: Props = $props();
</script>

{#if message}
  <p class="composer-error" role="alert">{message}</p>
{/if}

<style>
  .composer-error {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--ui-danger);
  }
</style>
