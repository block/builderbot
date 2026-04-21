<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowLeft, FolderGit2, Keyboard, Stethoscope } from 'lucide-svelte';
  import { closeSettings, navigation } from '../layout/navigation.svelte';
  import ActionsSettingsPanel from './ActionsSettingsPanel.svelte';
  import DoctorSettingsPanel from './DoctorSettingsPanel.svelte';
  import KeyboardSettingsPanel from './KeyboardSettingsPanel.svelte';

  let appVersion = $state(__APP_VERSION__);

  onMount(async () => {
    if (typeof window === 'undefined' || !('__TAURI__' in window)) return;

    try {
      const { getVersion } = await import('@tauri-apps/api/app');
      appVersion = await getVersion();
    } catch (error) {
      console.warn('[Settings] Could not load runtime app version', error);
    }
  });

  function handleBack() {
    closeSettings();
  }
</script>

<div class="settings-page">
  <header class="settings-header">
    <button class="back-btn" onclick={handleBack} title="Back to workspace">
      <ArrowLeft size={14} />
      Back
    </button>
    <div class="header-text">
      <h1>Settings</h1>
      <p class="header-meta">v{appVersion}</p>
    </div>
  </header>

  <div class="settings-body">
    <aside class="settings-nav" aria-label="Settings sections">
      <div class="settings-nav-list">
        <button
          class="nav-item"
          class:active={navigation.settingsSection === 'repo'}
          onclick={() => (navigation.settingsSection = 'repo')}
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
          class:active={navigation.settingsSection === 'keyboard'}
          onclick={() => (navigation.settingsSection = 'keyboard')}
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
          onclick={() => (navigation.settingsSection = 'doctor')}
        >
          <div class="nav-main">
            <Stethoscope size={14} />
            <div class="nav-text">
              <span class="nav-name">Doctor</span>
              <span class="nav-meta">Environment checks and setup</span>
            </div>
          </div>
        </button>
      </div>
    </aside>

    <section class="settings-content">
      {#if navigation.settingsSection === 'repo'}
        <ActionsSettingsPanel />
      {:else if navigation.settingsSection === 'keyboard'}
        <KeyboardSettingsPanel />
      {:else}
        <DoctorSettingsPanel />
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
    background: var(--bg-primary);
  }

  .settings-header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 16px;
    border-bottom: 1px solid color-mix(in srgb, var(--border-subtle) 60%, transparent);
    background: color-mix(in srgb, var(--bg-chrome) 82%, transparent);
  }

  .back-btn {
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 9px;
    cursor: pointer;
    flex-shrink: 0;
    font-size: var(--size-sm);
    transition:
      color 0.12s ease,
      background-color 0.12s ease,
      border-color 0.12s ease;
  }

  .back-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
    border-color: color-mix(in srgb, var(--border-subtle) 60%, transparent);
  }

  .header-text {
    min-width: 0;
  }

  .header-text h1 {
    margin: 0;
    font-size: calc(var(--size-xl) * 1.1);
    line-height: 1.2;
  }

  .header-meta {
    margin: 4px 0 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .settings-body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 220px 1fr;
    overflow: hidden;
  }

  .settings-nav {
    border-right: 1px solid color-mix(in srgb, var(--border-subtle) 60%, transparent);
    padding: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: color-mix(in srgb, var(--bg-chrome) 75%, transparent);
  }

  .settings-nav-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 0 8px 10px;
    overflow: auto;
  }

  .nav-item {
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--text-primary);
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 8px 10px;
    cursor: pointer;
    text-align: left;
    font-size: var(--size-sm);
    transition: all 0.15s ease;
  }

  .nav-item:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: -1px;
  }

  .nav-item:hover {
    background: var(--ui-selection);
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
    padding: 14px;
    overflow: hidden;
  }

  @media (max-width: 920px) {
    .settings-header {
      flex-direction: column;
      align-items: flex-start;
    }

    .settings-body {
      grid-template-columns: 1fr;
      grid-template-rows: auto 1fr;
    }

    .settings-nav {
      border-right: 0;
      border-bottom: 1px solid color-mix(in srgb, var(--border-subtle) 60%, transparent);
      background: color-mix(in srgb, var(--bg-chrome) 65%, transparent);
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
      min-width: max-content;
      padding: 8px 12px;
    }

    .nav-meta {
      display: none;
    }
  }
</style>
