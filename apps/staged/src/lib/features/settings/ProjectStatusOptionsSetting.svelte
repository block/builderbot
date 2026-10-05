<!--
  ProjectStatusOptionsSetting.svelte — edit the statuses a project can be set
  to from its detail page (see ProjectStatusPicker).

  One row per option: icon, label, color swatch and delete. Options render in
  stored order and new ones append. Projects store an option's id, so
  renaming or recoloring one updates every project using it, and deleting one
  returns those projects to Default.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Info from '@lucide/svelte/icons/info';
  import Plus from '@lucide/svelte/icons/plus';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import IconPicker from '../actions/IconPicker.svelte';
  import ProjectStatusOptionIcon from '../projects/ProjectStatusOptionIcon.svelte';
  import {
    NEW_PROJECT_STATUS_ICON,
    PROJECT_STATUS_COLORS,
    PROJECT_STATUS_COLOR_VARS,
    isDefaultProjectStatusOptions,
    type ProjectStatusOption,
  } from '../projects/projectStatusDisplay';
  import {
    preferences,
    resetProjectStatusOptions,
    setProjectStatusOptions,
  } from './preferences.svelte';

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

  let options = $derived(preferences.projectStatusOptions);
  /** Nothing to reset when the list already equals the defaults. */
  let isDefault = $derived(isDefaultProjectStatusOptions(options));

  // Resetting discards user-added statuses for good (their ids are UUIDs that
  // cannot be recreated) and returns projects using them to Default, so it
  // asks first.
  let showResetConfirm = $state(false);

  function confirmReset() {
    resetProjectStatusOptions();
    // AlertDialog.Action does not close the dialog itself (only Cancel does),
    // so every confirm handler closes it explicitly.
    showResetConfirm = false;
  }

  function labelInputId(option: ProjectStatusOption): string {
    return `project-status-label-${option.id}`;
  }

  function updateOption(id: string, patch: Partial<Omit<ProjectStatusOption, 'id'>>) {
    setProjectStatusOptions(options.map((o) => (o.id === id ? { ...o, ...patch } : o)));
  }

  function removeOption(id: string) {
    setProjectStatusOptions(options.filter((o) => o.id !== id));
  }

  async function addOption() {
    const option: ProjectStatusOption = {
      id: crypto.randomUUID(),
      label: '',
      icon: NEW_PROJECT_STATUS_ICON,
      color: 'gray',
    };
    setProjectStatusOptions([...options, option]);
    await tick();
    document.getElementById(labelInputId(option))?.focus();
  }
</script>

<div class="status-options-field">
  <span class="field-label">Project statuses</span>

  {#if options.length > 0}
    <ul class="status-option-list">
      {#each options as option (option.id)}
        <li class="status-option-row">
          <IconPicker
            icon={option.icon}
            curated={STATUS_ICONS}
            onSelect={(icon) => updateOption(option.id, { icon: icon ?? NEW_PROJECT_STATUS_ICON })}
          >
            {#snippet triggerIcon()}
              <ProjectStatusOptionIcon {option} size={14} />
            {/snippet}
          </IconPicker>
          <Input
            id={labelInputId(option)}
            type="text"
            placeholder="Status name"
            aria-label="Status name"
            class="min-w-0 flex-1"
            value={option.label}
            oninput={(e) => updateOption(option.id, { label: e.currentTarget.value })}
          />
          <div class="swatches" role="radiogroup" aria-label="Status color">
            {#each PROJECT_STATUS_COLORS as color (color)}
              <button
                type="button"
                class="swatch"
                class:selected={option.color === color}
                role="radio"
                aria-checked={option.color === color}
                aria-label={color}
                title={color}
                style:--swatch-color={PROJECT_STATUS_COLOR_VARS[color]}
                onclick={() => updateOption(option.id, { color })}
              ></button>
            {/each}
          </div>
          <Button
            variant="ghost"
            size="icon-sm"
            class="hover:text-destructive"
            title="Delete status"
            aria-label="Delete status"
            onclick={() => removeOption(option.id)}
          >
            <Trash2 size={13} />
          </Button>
        </li>
      {/each}
    </ul>
  {/if}

  <div class="status-option-actions">
    <Button variant="outline" size="sm" onclick={() => void addOption()}>
      <Plus size={14} />
      Add status
    </Button>
    <!--
      The title lives on the wrapper: a disabled Button has pointer-events
      none, so a title on the button itself would never show.
    -->
    <span class="inline-flex" title={isDefault ? 'Statuses already match the defaults' : undefined}>
      <Button
        variant="ghost"
        size="sm"
        disabled={isDefault}
        onclick={() => (showResetConfirm = true)}
      >
        <RotateCcw size={14} />
        Reset to defaults
      </Button>
    </span>
  </div>

  <p class="field-description">
    <Info size={12} />
    Set a status from a project's page to show it in place of the PR status. Deleting a status returns
    any project using it to Default.
  </p>
</div>

<AlertDialog.Root bind:open={showResetConfirm}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Reset Project Statuses</AlertDialog.Title>
      <AlertDialog.Description>
        Restore the built-in statuses? Statuses you added will be removed and cannot be recovered,
        and any project using one returns to Default.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={confirmReset}>Reset</AlertDialog.Action>
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
