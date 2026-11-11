<!--
  DeleteTagDialog.svelte — confirm deleting a tag, saying how many projects
  lose it.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import type { Tag } from '../../../types';

  interface Props {
    open: boolean;
    tag: Tag | null;
    /** Live projects carrying the tag. */
    usage: number;
    /** Whether the tag is in the icon group, so its projects fall back to their PR status. */
    inIconGroup: boolean;
    onConfirm: () => void;
  }

  let { open = $bindable(), tag, usage, inIconGroup, onConfirm }: Props = $props();

  let projects = $derived(usage === 1 ? '1 project' : `${usage} projects`);
</script>

<AlertDialog.Root bind:open>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete Tag</AlertDialog.Title>
      <AlertDialog.Description>
        Delete “{tag?.name}”?
        {#if usage === 0}
          No project uses it.
        {:else if inIconGroup}
          Used by {projects}, which
          {usage === 1 ? 'goes back to showing its' : 'go back to showing their'} PR status.
        {:else}
          Used by {projects}, which {usage === 1 ? 'loses' : 'lose'} it.
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={onConfirm}>Delete</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
