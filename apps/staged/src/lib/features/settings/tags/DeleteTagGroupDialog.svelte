<!--
  DeleteTagGroupDialog.svelte — confirm deleting a tag group, choosing what
  happens to its tags: kept as ungrouped tags (the default; projects keep
  them) or deleted with the group.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import type { DeleteTagGroupMode, TagGroup } from '../../../types';

  interface Props {
    open: boolean;
    group: TagGroup | null;
    tagCount: number;
    /** Live projects with a tag in the group. */
    usage: number;
    onConfirm: (mode: DeleteTagGroupMode) => void;
  }

  let { open = $bindable(), group, tagCount, usage, onConfirm }: Props = $props();

  let mode = $state<DeleteTagGroupMode>('ungroup');

  // Every opening starts from the safe choice.
  $effect.pre(() => {
    if (open) mode = 'ungroup';
  });

  let tags = $derived(tagCount === 1 ? 'its tag' : `its ${tagCount} tags`);
  let projects = $derived(usage === 1 ? '1 project loses' : `${usage} projects lose`);
</script>

<AlertDialog.Root bind:open>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete Group</AlertDialog.Title>
      <AlertDialog.Description>
        Delete “{group?.name}”?
        {#if tagCount === 0}
          It has no tags.
        {/if}
        {#if group?.isIconGroup}
          Projects will show their PR status icon instead.
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>

    {#if tagCount > 0}
      <fieldset class="modes">
        <legend class="sr-only">What happens to {tags}</legend>
        <label class="mode">
          <input type="radio" name="delete-tag-group-mode" value="ungroup" bind:group={mode} />
          <span class="mode-text">
            <span class="mode-title">Ungroup its tags</span>
            <span class="mode-detail">Keep {tags} as ungrouped tags. Projects keep them.</span>
          </span>
        </label>
        <label class="mode">
          <input type="radio" name="delete-tag-group-mode" value="deleteTags" bind:group={mode} />
          <span class="mode-text">
            <span class="mode-title">Delete group and tags</span>
            <span class="mode-detail">
              Delete {tags}.
              {usage === 0 ? 'No project uses them.' : `${projects} theirs.`}
            </span>
          </span>
        </label>
      </fieldset>
    {/if}

    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => onConfirm(tagCount === 0 ? 'deleteTags' : mode)}
      >
        Delete group
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .modes {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    border: none;
  }

  .mode {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 10px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    cursor: pointer;
    transition:
      border-color 0.1s ease,
      background-color 0.1s ease;
  }

  .mode:hover {
    background: var(--bg-hover);
  }

  .mode:has(input:checked) {
    border-color: var(--border-emphasis);
  }

  .mode input {
    margin: 2px 0 0;
    accent-color: var(--ui-accent);
  }

  .mode-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .mode-title {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .mode-detail {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }
</style>
