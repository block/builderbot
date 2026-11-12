<!--
  MoveTagConfirmDialog.svelte — confirm moving a tag into a group where some
  of its projects already have a tag. A group is single-choice, so those
  projects keep the tag they have and lose this one.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';

  interface Props {
    open: boolean;
    tagName: string;
    groupName: string;
    /** Projects carrying the tag that already have one in the group. */
    conflicts: number;
    onConfirm: () => void;
  }

  let { open = $bindable(), tagName, groupName, conflicts, onConfirm }: Props = $props();
</script>

<AlertDialog.Root bind:open>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Move Tag</AlertDialog.Title>
      <AlertDialog.Description>
        Move ‘{tagName}’ to {groupName}?
        {#if conflicts === 1}
          1 project already has a {groupName}. It keeps that and loses ‘{tagName}’.
        {:else}
          {conflicts} projects already have a {groupName}. They keep it and lose ‘{tagName}’.
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action onclick={onConfirm}>Move</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
