<script lang="ts" module>
  let pickerCounter = 0;
</script>

<!--
  TagPicker.svelte — the `+ Tag` button in a project's header and the list it
  opens (also opened by the T shortcut, through `open`).

  A search box over every tag: each group's tags act as radio buttons (picking
  the checked one clears it), the ungrouped tags as checkboxes. When nothing
  is named exactly what was typed, a create row makes it an ungrouped tag with
  an automatic colour, assigned to the project. Enter picks it when no tag
  matches; otherwise the highlight starts on the first match, so a prefix of
  an existing tag picks that tag instead of making a near-duplicate. Up/Down
  move the highlight, Enter toggles, Escape closes; the list stays open so
  several tags can be set in a row. "Manage tags…" opens Settings › Tags.

  A popover anchored to the button; in the mobile layout the same list fills
  a dialog instead. See tagPickerModel.ts for the rows.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Check from '@lucide/svelte/icons/check';
  import Plus from '@lucide/svelte/icons/plus';
  import Search from '@lucide/svelte/icons/search';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import * as Dialog from '$lib/components/ui/dialog';
  import * as Popover from '$lib/components/ui/popover';
  import type { Project } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import { autofocusOnOpen } from '../../shared/focusAfterDialogEnter';
  import { viewport } from '../../shared/viewport.svelte';
  import { openSettings } from '../layout/navigation.svelte';
  import TagIcon from './TagIcon.svelte';
  import { tagActions } from './tagActions.svelte';
  import { MAX_TAG_NAME_CHARS } from './tagState';
  import { buildTagPickerModel, moveActiveIndex, type TagPickerRow } from './tagPickerModel';

  interface Props {
    project: Project;
    open?: boolean;
    disabled?: boolean;
  }

  let { project, open = $bindable(false), disabled = false }: Props = $props();

  const listId = `tag-picker-${++pickerCounter}`;

  let query = $state('');
  let active = $state(0);
  let creating = $state(false);
  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLElement | null>(null);
  let wasOpen = false;

  let model = $derived(
    buildTagPickerModel(tagsStore.catalog, tagsStore.tagIdsFor(project.id), query)
  );
  let activeIndex = $derived(Math.min(active, model.rows.length - 1));
  let rowIndex = $derived(new Map(model.rows.map((row, index) => [row.key, index])));
  let manageIndex = $derived(model.rows.length - 1);
  let empty = $derived(tagsStore.tagsById.size === 0 && query.trim() === '');

  // Each opening starts from an empty search.
  $effect(() => {
    if (open && !wasOpen) {
      query = '';
      active = 0;
      if (viewport.isMobile) void tick().then(() => autofocusOnOpen(inputEl));
    }
    wasOpen = open;
  });

  function optionId(row: TagPickerRow): string {
    return `${listId}-${row.key}`;
  }

  function handleInput() {
    active = model.defaultActive;
  }

  function scrollActiveIntoView() {
    void tick().then(() => {
      listEl
        ?.querySelector<HTMLElement>('[data-active="true"]')
        ?.scrollIntoView?.({ block: 'nearest' });
    });
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      active = moveActiveIndex(activeIndex, event.key === 'ArrowDown' ? 1 : -1, model.rows.length);
      scrollActiveIntoView();
    } else if (event.key === 'Enter') {
      event.preventDefault();
      const row = model.rows[activeIndex];
      if (row) void activate(row);
    }
  }

  async function activate(row: TagPickerRow) {
    switch (row.kind) {
      case 'tag': {
        const { tag, checked } = row;
        if (tag.groupId !== null) {
          void tagActions.setProjectGroupTag(project, tag.groupId, checked ? null : tag.id);
        } else {
          void tagActions.setProjectTagAssigned(project, tag.id, !checked);
        }
        return;
      }
      case 'create': {
        if (row.tooLong || creating) return;
        creating = true;
        const created = await tagActions.createTagForProject(project, row.name);
        creating = false;
        if (!created) return;
        query = '';
        active = rowIndex.get(`tag:${created.id}`) ?? 0;
        scrollActiveIntoView();
        return;
      }
      case 'manage':
        open = false;
        openSettings('tags');
        return;
    }
  }
</script>

{#snippet body()}
  <div class="tag-picker">
    <label class="tag-picker-search">
      <Search size={13} />
      <input
        bind:this={inputEl}
        bind:value={query}
        type="text"
        role="combobox"
        placeholder="Search or create tags"
        aria-label="Search or create tags"
        aria-expanded="true"
        aria-controls={listId}
        aria-activedescendant={model.rows[activeIndex]
          ? optionId(model.rows[activeIndex])
          : undefined}
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck="false"
        oninput={handleInput}
        onkeydown={handleKeydown}
      />
    </label>

    <div class="tag-picker-list" id={listId} role="listbox" bind:this={listEl}>
      {#if empty}
        <div class="tag-picker-hint">No tags yet. Type a name to create one.</div>
      {/if}
      {#each model.sections as section (section.groupId ?? '')}
        <div
          class="tag-picker-section"
          role="group"
          aria-labelledby={`${listId}-section-${section.groupId ?? 'ungrouped'}`}
        >
          <div
            class="tag-picker-heading"
            id={`${listId}-section-${section.groupId ?? 'ungrouped'}`}
          >
            {section.label}
          </div>
          {#each section.rows as row (row.key)}
            {@const index = rowIndex.get(row.key) ?? -1}
            <!-- Focus stays in the search box (aria-activedescendant), so the
                 rows take clicks but never focus. -->
            <div
              id={optionId(row)}
              class="tag-picker-row"
              role="option"
              tabindex="-1"
              aria-selected={index === activeIndex}
              aria-checked={row.checked}
              data-active={index === activeIndex}
              data-tag-option={row.tag.id}
              onpointermove={() => (active = index)}
              onmousedown={(event) => event.preventDefault()}
              onclick={() => void activate(row)}
              onkeydown={handleKeydown}
            >
              <span
                class="tag-picker-indicator"
                class:radio={row.mode === 'radio'}
                class:checked={row.checked}
                aria-hidden="true"
              >
                {#if row.checked && row.mode === 'checkbox'}
                  <Check size={10} strokeWidth={3} />
                {/if}
              </span>
              <TagIcon tag={row.tag} size={14} />
              <span class="tag-picker-name">{row.tag.name}</span>
            </div>
          {/each}
        </div>
      {/each}

      {#if model.create}
        {@const row = model.create}
        {@const index = rowIndex.get(row.key) ?? -1}
        <div
          id={optionId(row)}
          class="tag-picker-row tag-picker-action"
          role="option"
          tabindex="-1"
          aria-selected={index === activeIndex}
          aria-disabled={row.tooLong || creating}
          data-active={index === activeIndex}
          onpointermove={() => (active = index)}
          onmousedown={(event) => event.preventDefault()}
          onclick={() => void activate(row)}
          onkeydown={handleKeydown}
        >
          <Plus size={14} />
          {#if row.tooLong}
            <span class="tag-picker-name">Names are at most {MAX_TAG_NAME_CHARS} characters</span>
          {:else}
            <span class="tag-picker-name">Create tag “{row.name}”</span>
          {/if}
        </div>
      {/if}

      <div class="tag-picker-separator" role="presentation"></div>
      <div
        id={optionId(model.manage)}
        class="tag-picker-row tag-picker-action"
        role="option"
        tabindex="-1"
        aria-selected={manageIndex === activeIndex}
        data-active={manageIndex === activeIndex}
        onpointermove={() => (active = manageIndex)}
        onmousedown={(event) => event.preventDefault()}
        onclick={() => void activate(model.manage)}
        onkeydown={handleKeydown}
      >
        <Settings2 size={14} />
        <span class="tag-picker-name">Manage tags…</span>
      </div>
    </div>
  </div>
{/snippet}

{#if viewport.isMobile}
  <button
    type="button"
    class="add-tag-button touch-hit"
    {disabled}
    title="Add a tag"
    onclick={() => (open = true)}
  >
    <Plus size={12} /> Tag
  </button>
  <Dialog.Root bind:open>
    <Dialog.Content class="gap-3">
      <Dialog.Header>
        <Dialog.Title>Tags</Dialog.Title>
      </Dialog.Header>
      {@render body()}
    </Dialog.Content>
  </Dialog.Root>
{:else}
  <Popover.Root bind:open>
    <Popover.Trigger class="add-tag-button touch-hit" {disabled} title="Add a tag">
      <Plus size={12} /> Tag
    </Popover.Trigger>
    <Popover.Content
      align="start"
      sideOffset={6}
      class="w-[260px] gap-0 p-1.5"
      onOpenAutoFocus={(event) => {
        event.preventDefault();
        inputEl?.focus();
      }}
    >
      {@render body()}
    </Popover.Content>
  </Popover.Root>
{/if}

<style>
  :global(.add-tag-button) {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    flex-shrink: 0;
    padding: 2px 8px 2px 6px;
    border: 1px dashed var(--border-muted);
    border-radius: 999px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    line-height: 1.4;
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease,
      color 0.15s ease;
  }

  :global(.add-tag-button:hover:not(:disabled)) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
    color: var(--text-secondary);
  }

  :global(.add-tag-button:disabled) {
    cursor: default;
    opacity: 0.6;
  }

  .tag-picker {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .tag-picker-search {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 6px;
    padding: 4px 6px 6px;
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-faint);
  }

  .tag-picker-search input {
    min-width: 0;
    padding: 2px 0;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text-primary);
    font-size: var(--size-sm);
  }

  .tag-picker-search input::placeholder {
    color: var(--text-faint);
  }

  .tag-picker-list {
    display: flex;
    flex-direction: column;
    max-height: 320px;
    padding-top: 4px;
    overflow-y: auto;
  }

  .tag-picker-heading {
    padding: 6px 6px 2px;
    color: var(--text-faint);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .tag-picker-row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    padding: 5px 6px;
    border-radius: 4px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    user-select: none;
  }

  .tag-picker-row[data-active='true'] {
    background: var(--bg-hover);
  }

  .tag-picker-row[aria-disabled='true'] {
    color: var(--text-muted);
    cursor: default;
  }

  .tag-picker-row :global(svg) {
    flex-shrink: 0;
  }

  .tag-picker-action {
    color: var(--text-secondary);
  }

  .tag-picker-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* A radio dot for a group's tags, a checkbox for the ungrouped ones. */
  .tag-picker-indicator {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 12px;
    height: 12px;
    border: 1.5px solid var(--border-emphasis);
    border-radius: 3px;
    color: var(--bg-primary);
  }

  .tag-picker-indicator.radio {
    border-radius: 999px;
  }

  .tag-picker-indicator.checked {
    border-color: var(--ui-accent);
    background: var(--ui-accent);
  }

  .tag-picker-indicator.radio.checked {
    background: radial-gradient(circle, var(--ui-accent) 0 2.5px, transparent 3px);
  }

  .tag-picker-separator {
    height: 1px;
    margin: 4px 2px;
    background: var(--border-subtle);
  }

  .tag-picker-hint {
    padding: 8px 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  @media (pointer: coarse) {
    .tag-picker-row {
      min-height: 40px;
    }

    .tag-picker-list {
      max-height: none;
    }

    .tag-picker-search input {
      font-size: 16px;
    }
  }
</style>
