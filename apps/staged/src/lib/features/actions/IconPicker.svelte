<!--
  IconPicker.svelte — pick the Lucide icon an action's header button shows.

  The trigger is a button showing the current icon; opening it loads the full
  icon map (one lazy chunk, see lucideIcons.ts) and shows a search box over a
  grid. An empty query gets the curated shortlist; anything else substring-
  matches the kebab-case names, capped so the grid never tries to paint 1,600
  SVGs. "Default" clears back to the action type's own icon.
-->
<script lang="ts">
  import Search from '@lucide/svelte/icons/search';
  import * as Popover from '$lib/components/ui/popover';
  import { Input } from '$lib/components/ui/input';
  import Spinner from '../../shared/Spinner.svelte';
  import ActionIcon from './ActionIcon.svelte';
  import { ICON_SEARCH_LIMIT, searchIconNames } from './iconNames';
  import { getActionTypeIcon, loadIconMap, type IconComponent } from './lucideIcons';

  interface Props {
    /** Currently selected kebab-case icon name, or null for the type default. */
    icon: string | null;
    /** Which default the "Default" entry stands for. */
    actionType: string;
    onSelect: (icon: string | null) => void;
  }

  let { icon, actionType, onSelect }: Props = $props();

  let open = $state(false);
  let query = $state('');
  let iconMap = $state<Record<string, IconComponent> | null>(null);

  // Opening is what pays for the icon chunk — nothing loads for a settings
  // page the user never opens the picker on.
  $effect(() => {
    if (!open || iconMap) return;
    loadIconMap().then((map) => {
      iconMap = map;
    });
  });

  let allNames = $derived(iconMap ? Object.keys(iconMap).sort() : []);
  let results = $derived(searchIconNames(allNames, query));
  let truncated = $derived(query.trim().length > 0 && results.length === ICON_SEARCH_LIMIT);
  let TypeIcon = $derived(getActionTypeIcon(actionType));
</script>

<Popover.Root bind:open>
  <Popover.Trigger
    class="icon-picker-trigger"
    title="Choose icon"
    aria-label="Choose icon"
    onclick={() => (query = '')}
  >
    <ActionIcon {icon} {actionType} size={14} />
  </Popover.Trigger>
  <Popover.Content align="start" sideOffset={6} class="w-[268px] p-2">
    <label class="icon-search">
      <Search size={13} />
      <Input bind:value={query} placeholder="Search icons" aria-label="Search icons" />
    </label>

    <button
      class="icon-default"
      class:selected={icon === null}
      onclick={() => {
        onSelect(null);
        open = false;
      }}
    >
      <TypeIcon size={14} />
      Default for {actionType}
    </button>

    {#if !iconMap}
      <div class="icon-loading"><Spinner size={14} /> Loading icons…</div>
    {:else if results.length === 0}
      <div class="icon-empty">No icons match "{query.trim()}"</div>
    {:else}
      <div class="icon-grid">
        {#each results as name (name)}
          {@const Icon = iconMap[name]}
          <button
            class="icon-option"
            class:selected={icon === name}
            title={name}
            aria-label={name}
            onclick={() => {
              onSelect(name);
              open = false;
            }}
          >
            <Icon size={16} />
          </button>
        {/each}
      </div>
      {#if truncated}
        <div class="icon-hint">Showing the first {ICON_SEARCH_LIMIT} — keep typing to narrow.</div>
      {/if}
    {/if}
  </Popover.Content>
</Popover.Root>

<style>
  :global(.icon-picker-trigger) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  :global(.icon-picker-trigger:hover) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  .icon-search {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 6px;
    color: var(--text-faint);
    margin-bottom: 6px;
  }

  .icon-default {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
  }

  .icon-default:hover {
    background: var(--bg-hover);
  }

  .icon-default.selected {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .icon-grid {
    display: grid;
    grid-template-columns: repeat(8, minmax(0, 1fr));
    gap: 2px;
    max-height: 200px;
    overflow-y: auto;
    margin-top: 4px;
  }

  .icon-option {
    display: flex;
    align-items: center;
    justify-content: center;
    aspect-ratio: 1;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .icon-option:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .icon-option.selected {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
  }

  .icon-loading,
  .icon-empty,
  .icon-hint {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 10px 6px 4px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    text-align: center;
  }
</style>
