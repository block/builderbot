<!--
  BoardAddColumn.svelte — the trailing + column: adds a tag to the board's
  group, which becomes a new column. The tag gets an automatic colour and no
  icon; both can be changed from the column menu or Settings › Tags.
-->
<script lang="ts">
  import Plus from '@lucide/svelte/icons/plus';
  import { tick } from 'svelte';
  import { Button } from '$lib/components/ui/button';
  import type { TagGroup } from '../../types';
  import { tagActions } from '../tags/tagActions.svelte';
  import { MAX_TAG_NAME_CHARS } from '../tags/tagState';

  interface Props {
    group: TagGroup;
    /** On the phone pager: full width. */
    paged: boolean;
  }

  let { group, paged }: Props = $props();

  let adding = $state(false);
  let saving = $state(false);
  let name = $state('');
  let input = $state<HTMLInputElement | null>(null);

  async function open() {
    adding = true;
    await tick();
    input?.focus();
  }

  function close() {
    adding = false;
    name = '';
  }

  async function create(event: SubmitEvent) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed || saving) return;
    saving = true;
    const tag = await tagActions.createTag({ name: trimmed, groupId: group.id });
    saving = false;
    // On failure (already toasted) the name stays, to fix and retry.
    if (tag) close();
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
    }
  }

  function handleFocusOut(event: FocusEvent & { currentTarget: HTMLFormElement }) {
    // Leaving the form with nothing typed closes it; a typed name stays.
    const next = event.relatedTarget;
    if (next instanceof Node && event.currentTarget.contains(next)) return;
    if (!name.trim() && !saving) close();
  }
</script>

<div class="board-add-column" class:paged>
  {#if adding}
    <form class="add-form" onsubmit={create} onfocusout={handleFocusOut}>
      <input
        bind:this={input}
        bind:value={name}
        class="add-input"
        type="text"
        placeholder={`New ${group.name} tag`}
        aria-label={`New ${group.name} tag`}
        maxlength={MAX_TAG_NAME_CHARS}
        disabled={saving}
        onkeydown={handleKeydown}
      />
      <div class="add-actions">
        <Button type="submit" size="sm" disabled={saving || !name.trim()}>Add</Button>
        <Button type="button" variant="ghost" size="sm" onclick={close}>Cancel</Button>
      </div>
    </form>
  {:else}
    <button
      type="button"
      class="add-trigger"
      title={`Add a ${group.name} tag`}
      aria-label={`Add a ${group.name} tag`}
      onclick={() => void open()}
    >
      <Plus size={16} />
    </button>
  {/if}
</div>

<style>
  .board-add-column {
    flex: 0 0 auto;
    display: flex;
  }

  .board-add-column.paged {
    flex-basis: 100%;
    scroll-snap-align: start;
  }

  .add-trigger {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 42px;
    border: 1px dashed var(--border-muted);
    border-radius: 10px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .paged .add-trigger {
    width: 100%;
  }

  .add-trigger:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .add-form {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 272px;
    padding: 8px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-primary) 45%, transparent);
  }

  .paged .add-form {
    width: 100%;
  }

  .add-input {
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--border-emphasis);
    border-radius: 6px;
    background: var(--bg-input);
    color: var(--text-primary);
    font-size: var(--size-sm);
    outline: none;
  }

  .add-input:focus {
    border-color: var(--ui-accent);
  }

  .add-actions {
    display: flex;
    gap: 4px;
  }
</style>
