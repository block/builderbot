<script lang="ts">
  import { onMount } from 'svelte';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import Keyboard from '@lucide/svelte/icons/keyboard';
  import Plug from '@lucide/svelte/icons/plug';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import Stethoscope from '@lucide/svelte/icons/stethoscope';
  import TagsIcon from '@lucide/svelte/icons/tags';
  import { navigation, openSettings } from '../layout/navigation.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';
  import ActionsSettingsPanel from './ActionsSettingsPanel.svelte';
  import ConnectedAppsPanel from './ConnectedAppsPanel.svelte';
  import DoctorSettingsPanel from './DoctorSettingsPanel.svelte';
  import GeneralSettingsPanel from './GeneralSettingsPanel.svelte';
  import KeyboardSettingsPanel from './KeyboardSettingsPanel.svelte';
  import TagsSettingsPanel from './tags/TagsSettingsPanel.svelte';
  import { tagsStore } from '../../stores/tags.svelte';
  import { isTauri } from '../../transport';

  let appVersion = $state(__APP_VERSION__);

  let groupCount = $derived(tagsStore.groups.length);
  let tagsMeta = $derived(
    !tagsStore.loaded
      ? 'Project tags and groups'
      : groupCount === 0
        ? 'No groups yet'
        : `${groupCount} ${groupCount === 1 ? 'group' : 'groups'}`
  );

  onMount(async () => {
    if (!isTauri) return;

    try {
      const { getVersion } = await import('@tauri-apps/api/app');
      appVersion = await getVersion();
    } catch (error) {
      console.warn('[Settings] Could not load runtime app version', error);
    }
  });
</script>

<TopBarPortal title="Settings" subtitle={`v${appVersion}`} />

<div class="settings-page">
  <div class="settings-body">
    <aside class="settings-nav" aria-label="Settings sections">
      <div class="settings-nav-list">
        <button
          class="nav-item"
          class:active={navigation.settingsSection === 'general'}
          onclick={() => openSettings('general')}
        >
          <div class="nav-main">
            <Settings2 size={14} />
            <div class="nav-text">
              <span class="nav-name">General</span>
              <span class="nav-meta">Theme and appearance</span>
            </div>
          </div>
        </button>
        <button
          class="nav-item"
          class:active={navigation.settingsSection === 'repo'}
          onclick={() => openSettings('repo')}
        >
          <div class="nav-main">
            <FolderGit2 size={14} />
            <div class="nav-text">
              <span class="nav-name">Repos</span>
              <span class="nav-meta">Per-repo actions and cleanup</span>
            </div>
          </div>
        </button>
        <button
          class="nav-item"
          class:active={navigation.settingsSection === 'tags'}
          onclick={() => openSettings('tags')}
        >
          <div class="nav-main">
            <TagsIcon size={14} />
            <div class="nav-text">
              <span class="nav-name">Tags</span>
              <span class="nav-meta">{tagsMeta}</span>
            </div>
          </div>
        </button>
        <button
          class="nav-item"
          class:active={navigation.settingsSection === 'keyboard'}
          onclick={() => openSettings('keyboard')}
        >
          <div class="nav-main">
            <Keyboard size={14} />
            <div class="nav-text">
              <span class="nav-name">Keyboard</span>
              <span class="nav-meta">Global shortcuts and keybindings</span>
            </div>
          </div>
        </button>
        <button
          class="nav-item"
          class:active={navigation.settingsSection === 'doctor'}
          onclick={() => openSettings('doctor')}
        >
          <div class="nav-main">
            <Stethoscope size={14} />
            <div class="nav-text">
              <span class="nav-name">Doctor</span>
              <span class="nav-meta">Environment checks and setup</span>
            </div>
          </div>
        </button>
        {#if isTauri}
          <button
            class="nav-item"
            class:active={navigation.settingsSection === 'connected-apps'}
            onclick={() => openSettings('connected-apps')}
          >
            <div class="nav-main">
              <Plug size={14} />
              <div class="nav-text">
                <span class="nav-name">Connected apps</span>
                <span class="nav-meta">Tools approved to use stagedctl</span>
              </div>
            </div>
          </button>
        {/if}
      </div>
    </aside>

    <section class="settings-content">
      {#if navigation.settingsSection === 'repo'}
        <ActionsSettingsPanel />
      {:else}
        <div class="panel-pad">
          {#if navigation.settingsSection === 'general'}
            <GeneralSettingsPanel />
          {:else if navigation.settingsSection === 'tags'}
            <TagsSettingsPanel />
          {:else if navigation.settingsSection === 'keyboard'}
            <KeyboardSettingsPanel />
          {:else if navigation.settingsSection === 'connected-apps'}
            <ConnectedAppsPanel />
          {:else}
            <DoctorSettingsPanel />
          {/if}
        </div>
      {/if}
    </section>
  </div>
</div>

<style>
  .settings-page {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-chrome);
  }

  .settings-body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 220px 1fr;
    overflow: hidden;
  }

  .settings-nav {
    --settings-nav-hover-bg: color-mix(in srgb, var(--text-primary) 4%, transparent);

    border-right: 1px solid color-mix(in srgb, var(--border-subtle) 60%, transparent);
    padding: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg-app-bar);
  }

  .settings-nav-list {
    --settings-row-bleed: 8px;

    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 0 var(--settings-row-bleed) 10px;
    overflow: auto;
  }

  .nav-item {
    border: none;
    border-radius: 0;
    background: transparent;
    color: var(--text-primary);
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: calc(100% + (2 * var(--settings-row-bleed)));
    margin: 0 calc(-1 * var(--settings-row-bleed));
    padding: 8px 10px;
    cursor: pointer;
    text-align: left;
    font-size: var(--size-sm);
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .nav-item:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: -1px;
  }

  .nav-item:hover {
    background: var(--settings-nav-hover-bg);
  }

  .nav-item.active {
    background: var(--bg-hover);
  }

  .nav-main {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    flex: 1;
    min-width: 0;
  }

  .nav-main :global(svg) {
    flex-shrink: 0;
    width: 16px;
    color: inherit;
    stroke: currentColor;
  }

  .nav-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .nav-name {
    font-size: var(--size-sm);
    font-weight: 600;
    color: inherit;
    line-height: 1.2;
  }

  .nav-meta {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .nav-item.active .nav-meta {
    color: var(--text-muted);
  }

  .settings-content {
    min-height: 0;
    min-width: 0;
    display: grid;
    grid-template-rows: minmax(0, 1fr);
    grid-template-columns: minmax(0, 1fr);
    overflow: hidden;
  }

  /* Panels share the single cell so a deselected repos panel stays stacked
     under the incoming panel while its sidebar slides out. */
  .settings-content > :global(*) {
    grid-area: 1 / 1;
    min-width: 0;
    min-height: 0;
  }

  /* The panels are cards on the page's chrome; the bottom inset keeps them
     clear of the home indicator (App.svelte's main leaves it to the panes).
     On a phone (below) the card is dropped and the inset moves into the
     panel body's padding through `--settings-panel-inset-bottom`, so the
     body scrolls under the home indicator instead of stopping at a band of
     chrome above it. */
  .panel-pad {
    padding: 14px;
    padding-bottom: calc(14px + env(safe-area-inset-bottom));
    overflow: hidden;
    --settings-panel-inset-bottom: 0px;
  }

  @media (max-width: 920px) {
    /* `1fr` alone is `minmax(auto, 1fr)`: the nav row's `max-content` items
       set the column's minimum, and on a 402px phone the column (and every
       panel in it) grew to 413px, past the viewport. `minmax(0, 1fr)` keeps
       it at the width available, and the nav list scrolls sideways when the
       tabs don't fit. */
    .settings-body {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto 1fr;
    }

    .settings-nav {
      min-width: 0;
      border-right: 0;
      border-bottom: 1px solid color-mix(in srgb, var(--border-subtle) 60%, transparent);
      background: var(--bg-app-bar);
    }

    .settings-nav-list {
      flex-direction: row;
      align-items: center;
      padding: 8px;
      gap: 6px;
      overflow-x: auto;
    }

    .nav-item {
      width: auto;
      margin: 0;
      min-width: max-content;
      padding: 8px 10px;
    }

    .nav-meta {
      display: none;
    }

    /* Full-bleed panel: the nav row above is the only chrome, the panel
       runs to the screen's edges and its body takes the bottom inset. */
    .panel-pad {
      padding: 0;
      --settings-panel-inset-bottom: env(safe-area-inset-bottom);
    }

    /* Each panel's root sets its own border and radius at (0,2,0) specificity
       (`.general-settings-panel.svelte-x`), the same as a bare
       `.panel-pad > :global(*)` here. That tie is decided by bundle order,
       which only happens to favour this file; the extra ancestor makes this
       rule win on specificity so lazy-loading a panel cannot regress it. */
    .settings-body .panel-pad > :global(*) {
      border: 0;
      border-radius: 0;
    }
  }

  /* A finger-sized tab row: the tabs are 32px tall at their padding. */
  @media (max-width: 920px) and (pointer: coarse) {
    .nav-item {
      min-height: 44px;
    }
  }
</style>
