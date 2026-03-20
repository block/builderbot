<!--
  OpenInDropdown.svelte — "Open In" dropdown for diff pane headers.
  Shows available editors that can open the current file.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ExternalLink, ChevronDown, Copy } from 'lucide-svelte';
  import {
    getAvailableOpeners,
    openInApp,
    copyPathToClipboard,
    type OpenerApp,
  } from '../branches/branch';

  interface Props {
    /** Full absolute path to the file to open. */
    filePath: string | null;
  }

  let { filePath }: Props = $props();

  /** Apps that can open files (editors only, not terminals/finder). */
  const DIRECTORY_ONLY_IDS = new Set([
    'terminal',
    'warp',
    'iterm',
    'hyper',
    'kitty',
    'alacritty',
    'finder',
  ]);

  let fileOpenerApps = $state<OpenerApp[]>([]);
  let showDropdown = $state(false);

  onMount(() => {
    getAvailableOpeners().then((apps) => {
      fileOpenerApps = apps.filter((app) => !DIRECTORY_ONLY_IDS.has(app.id));
    });
  });

  function handleOpenIn(appId: string) {
    if (filePath) {
      openInApp(filePath, appId);
    }
    showDropdown = false;
  }

  function handleCopyPath() {
    if (filePath) {
      copyPathToClipboard(filePath);
    }
    showDropdown = false;
  }

  function handleToggle(e: MouseEvent) {
    e.stopPropagation();
    showDropdown = !showDropdown;
  }

  function handleClickOutside(e: MouseEvent) {
    if (showDropdown) {
      showDropdown = false;
    }
  }
</script>

<svelte:window onclick={handleClickOutside} />

{#if filePath && fileOpenerApps.length > 0}
  <div class="open-in-dropdown">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <button class="open-in-trigger" onclick={handleToggle} title="Open in...">
      <ExternalLink size={12} />
      <span>Open In</span>
      <ChevronDown size={10} />
    </button>

    {#if showDropdown}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="open-in-menu" onclick={(e) => e.stopPropagation()}>
        {#each fileOpenerApps as app (app.id)}
          <button class="open-in-item" onclick={() => handleOpenIn(app.id)}>
            {app.name}
          </button>
        {/each}
        <div class="menu-separator"></div>
        <button class="open-in-item" onclick={handleCopyPath}>
          <Copy size={12} />
          Copy Path
        </button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .open-in-dropdown {
    position: relative;
    margin-left: auto;
    flex-shrink: 0;
  }

  .open-in-trigger {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 6px;
    background: none;
    border: 1px solid var(--border-default);
    border-radius: 4px;
    color: var(--text-muted);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s,
      border-color 0.1s;
    white-space: nowrap;
  }

  .open-in-trigger:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
    border-color: var(--border-hover);
  }

  .open-in-menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    min-width: 160px;
    background: var(--bg-primary);
    border: 1px solid var(--border-default);
    border-radius: 8px;
    padding: 4px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    z-index: 100;
  }

  .open-in-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 8px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    text-align: left;
    white-space: nowrap;
  }

  .open-in-item:hover {
    background-color: var(--bg-hover);
  }

  .menu-separator {
    height: 1px;
    background: var(--border-default);
    margin: 4px 0;
  }
</style>
