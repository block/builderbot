<script lang="ts">
  import { ArrowLeft, Play } from 'lucide-svelte';
  import { closeSettings } from '../../navigation.svelte';
  import ActionsSettingsPanel from './ActionsSettingsPanel.svelte';

  let section = $state<'actions'>('actions');

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
      <p>Manage workspace preferences and action automation.</p>
    </div>
  </header>

  <div class="settings-body">
    <aside class="settings-nav" aria-label="Settings sections">
      <button
        class="nav-item"
        class:active={section === 'actions'}
        onclick={() => (section = 'actions')}
      >
        <Play size={14} />
        Actions
      </button>
    </aside>

    <section class="settings-content">
      <ActionsSettingsPanel />
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
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-primary);
    color: var(--text-primary);
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    cursor: pointer;
    flex-shrink: 0;
  }

  .back-btn:hover {
    background: var(--bg-hover);
  }

  .header-text {
    min-width: 0;
  }

  .header-text h1 {
    margin: 0;
    font-size: calc(var(--size-xl) * 1.1);
    line-height: 1.2;
  }

  .header-text p {
    margin: 3px 0 0;
    color: var(--text-muted);
    font-size: var(--size-sm);
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
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .nav-item {
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    cursor: pointer;
    text-align: left;
    font-size: var(--size-sm);
  }

  .nav-item:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .nav-item.active {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-chrome);
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
      flex-direction: row;
      align-items: center;
      overflow-x: auto;
    }
  }
</style>
