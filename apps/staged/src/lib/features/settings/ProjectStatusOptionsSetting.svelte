<!--
  ProjectStatusOptionsSetting.svelte — edit the statuses a project can be set
  to from its detail page (see ProjectStatusPicker): the tags of the icon tag
  group. An interim editor until Settings › Tags replaces it.

  One row per tag: icon, name, colour swatch and delete. Tags render in group
  order and new ones append. Projects carry a tag by id, so renaming or
  recolouring one updates every project using it, and deleting one returns
  those projects to Default. Each edit is a backend write, so a name saves on
  blur or Enter rather than per keystroke.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Info from '@lucide/svelte/icons/info';
  import Plus from '@lucide/svelte/icons/plus';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import type { Tag } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import IconPicker from '../actions/IconPicker.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { tagActions, type TagValue } from '../tags/tagActions.svelte';
  import { TAG_COLORS, TAG_COLOR_VARS } from '../tags/tagColors';

  /** The picker's empty-search shortlist: status-flavoured, not action-flavoured. */
  const STATUS_ICONS = [
    'lightbulb',
    'circle-dot',
    'paintbrush',
    'eye',
    'circle-check',
    'circle-pause',
    'octagon-minus',
    'clock',
    'flag',
    'bug',
    'rocket',
    'hourglass',
    'shield-alert',
  ];

  /** The icon a new, user-added status starts with. */
  const NEW_STATUS_ICON = 'circle-dot';

  let group = $derived(tagsStore.iconGroup);
  let tags = $derived(group ? (tagsStore.tagsByGroup.get(group.id) ?? []) : []);

  let adding = $state(false);

  // Deleting a status takes it off every project using it, so it asks first
  // and says how many that is. The target outlives the dialog's close so its
  // text doesn't blank mid-animation.
  let showDeleteConfirm = $state(false);
  let deleteTarget = $state<Tag | null>(null);
  let deleteTargetUsage = $derived(
    deleteTarget ? (tagsStore.usageCount.get(deleteTarget.id) ?? 0) : 0
  );

  function labelInputId(tag: Tag): string {
    return `project-status-label-${tag.id}`;
  }

  function save(tag: Tag, patch: Partial<TagValue>): Promise<boolean> {
    return tagActions.updateTag(tag, {
      name: tag.name,
      color: tag.color,
      icon: tag.icon,
      ...patch,
    });
  }

  async function commitName(tag: Tag, input: HTMLInputElement) {
    const name = input.value.trim();
    // A name can't be blank; an unchanged one needs no write.
    if (!name || name === tag.name) {
      input.value = tag.name;
      return;
    }
    if (!(await save(tag, { name }))) input.value = tag.name;
  }

  function handleNameKeydown(tag: Tag, event: KeyboardEvent & { currentTarget: HTMLInputElement }) {
    if (event.key === 'Enter') {
      event.currentTarget.blur();
    } else if (event.key === 'Escape') {
      event.currentTarget.value = tag.name;
      event.currentTarget.blur();
    }
  }

  /** "New status", or the first "New status N" the group doesn't have yet. */
  function newStatusName(): string {
    const taken = new Set(tags.map((tag) => tag.name.toLowerCase()));
    if (!taken.has('new status')) return 'New status';
    for (let n = 2; ; n++) {
      if (!taken.has(`new status ${n}`)) return `New status ${n}`;
    }
  }

  async function addStatus() {
    if (!group || adding) return;
    adding = true;
    try {
      const tag = await tagActions.createTag({
        name: newStatusName(),
        groupId: group.id,
        icon: NEW_STATUS_ICON,
      });
      if (!tag) return;
      await tick();
      const input = document.getElementById(labelInputId(tag));
      if (input instanceof HTMLInputElement) {
        input.focus();
        input.select();
      }
    } finally {
      adding = false;
    }
  }

  function requestDelete(tag: Tag) {
    deleteTarget = tag;
    showDeleteConfirm = true;
  }

  async function confirmDelete() {
    // AlertDialog.Action does not close the dialog itself (only Cancel does),
    // so every confirm handler closes it explicitly.
    showDeleteConfirm = false;
    if (deleteTarget) await tagActions.deleteTag(deleteTarget);
  }
</script>

<div class="status-options-field">
  <span class="field-label">Project statuses</span>

  {#if !group}
    <p class="field-description">
      <Info size={12} />
      There is no status group, so every project shows its PR status.
    </p>
  {:else}
    {#if tags.length > 0}
      <ul class="status-option-list">
        {#each tags as tag (tag.id)}
          <li class="status-option-row">
            <IconPicker
              icon={tag.icon}
              curated={STATUS_ICONS}
              onSelect={(icon) => {
                if (icon !== tag.icon) void save(tag, { icon });
              }}
            >
              {#snippet triggerIcon()}
                <TagIcon {tag} size={14} />
              {/snippet}
            </IconPicker>
            <Input
              id={labelInputId(tag)}
              type="text"
              placeholder="Status name"
              aria-label="Status name"
              class="min-w-0 flex-1"
              maxlength={64}
              value={tag.name}
              onblur={(e) => void commitName(tag, e.currentTarget)}
              onkeydown={(e) => handleNameKeydown(tag, e)}
            />
            <div class="swatches" role="radiogroup" aria-label="Status color">
              {#each TAG_COLORS as color (color)}
                <button
                  type="button"
                  class="swatch"
                  class:selected={tag.color === color}
                  role="radio"
                  aria-checked={tag.color === color}
                  aria-label={color}
                  title={color}
                  style:--swatch-color={TAG_COLOR_VARS[color]}
                  onclick={() => {
                    if (color !== tag.color) void save(tag, { color });
                  }}
                ></button>
              {/each}
            </div>
            <Button
              variant="ghost"
              size="icon-sm"
              class="hover:text-destructive"
              title="Delete status"
              aria-label="Delete status"
              onclick={() => requestDelete(tag)}
            >
              <Trash2 size={13} />
            </Button>
          </li>
        {/each}
      </ul>
    {/if}

    <div class="status-option-actions">
      <Button variant="outline" size="sm" disabled={adding} onclick={() => void addStatus()}>
        <Plus size={14} />
        Add status
      </Button>
    </div>

    <p class="field-description">
      <Info size={12} />
      Set a status from a project's page to show it in place of the PR status. Deleting a status returns
      any project using it to Default.
    </p>
  {/if}
</div>

<AlertDialog.Root bind:open={showDeleteConfirm}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete Status</AlertDialog.Title>
      <AlertDialog.Description>
        Delete “{deleteTarget?.name}”?
        {#if deleteTargetUsage === 0}
          No project uses it.
        {:else}
          Used by {deleteTargetUsage}
          {deleteTargetUsage === 1 ? 'project, which returns' : 'projects, which return'} to Default.
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={() => void confirmDelete()}>
        Delete
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .status-options-field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .field-label {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .status-option-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 560px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .status-option-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .swatches {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .swatch {
    width: 16px;
    height: 16px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 50%;
    background: var(--swatch-color);
    background-clip: content-box;
    cursor: pointer;
    transition: border-color 0.1s ease;
  }

  .swatch:hover {
    border-color: var(--border-emphasis);
  }

  .swatch.selected {
    border-color: var(--swatch-color);
    padding: 2px;
  }

  /* On a touch screen each swatch gets a 30px box (the dot stays 12px, drawn
     inside the padding), the delete button a 32px one, and the row may wrap
     so they fit a phone. */
  @media (pointer: coarse) {
    .status-option-row {
      flex-wrap: wrap;
    }

    /* The name field keeps a readable width and the swatches wrap under it
       rather than squeezing it to a few letters beside them (seven 30px
       swatches left it about 48px on a 402px phone). */
    .status-option-row > :global(input) {
      flex-basis: 160px;
    }

    .swatches {
      gap: 4px;
    }

    .swatch {
      width: 30px;
      height: 30px;
      padding: 7px;
    }

    .swatch.selected {
      padding: 9px;
    }

    .status-option-actions :global(button),
    .status-option-row > :global([data-slot='button']) {
      min-width: 32px;
      min-height: 32px;
    }
  }

  .status-option-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .field-description {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
    display: flex;
    align-items: baseline;
    gap: 4px;
  }
</style>
