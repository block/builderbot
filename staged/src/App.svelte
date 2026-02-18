<!--
  App.svelte — Root shell for Staged

  Initializes preferences (which loads the syntax theme and applies
  adaptive CSS variables), then renders the top bar and main content.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import * as commands from './lib/commands';
  import TopBar from './lib/TopBar.svelte';
  import ProjectHome from './lib/features/projects/ProjectHome.svelte';
  import ProjectsList from './lib/features/projects/ProjectsList.svelte';
  import SessionLauncher from './lib/features/sessions/SessionLauncher.svelte';
  import DoctorModal from './lib/features/doctor/DoctorModal.svelte';
  import ActionsPreferencesModal from './lib/features/settings/ActionsPreferencesModal.svelte';
  import ToastHost from './lib/shared/ToastHost.svelte';
  import { preferences, initPreferences } from './lib/features/settings/preferences.svelte';
  import { refreshProviders } from './lib/features/agents/agent.svelte';
  import { refreshSqAvailability } from './lib/features/settings/sq.svelte';
  import { navigation } from './lib/navigation.svelte';
  import { projectStateStore } from './lib/stores/projectState.svelte';
  import type { StoreIncompatibility } from './lib/types';

  let showSessionLab = $state(false);
  let showDoctor = $state(false);
  let showActionsPreferences = $state(false);
  let unlistenDoctor: UnlistenFn | undefined;
  let unlistenSessionStatus: UnlistenFn | undefined;
  let storeIncompat = $state<StoreIncompatibility | null>(null);
  let resetting = $state(false);
  let storeError = $state<string | null>(null);
  let onOpenActionsPreferences: (() => void) | null = null;

  // Konami code: ↑↑↓↓←→←→BA
  const konamiSequence = [
    'ArrowUp',
    'ArrowUp',
    'ArrowDown',
    'ArrowDown',
    'ArrowLeft',
    'ArrowRight',
    'ArrowLeft',
    'ArrowRight',
    'b',
    'a',
  ];
  let konamiIndex = 0;

  function handleKonamiKey(e: KeyboardEvent) {
    if (e.key === konamiSequence[konamiIndex]) {
      konamiIndex++;
      if (konamiIndex === konamiSequence.length) {
        konamiIndex = 0;
        showSessionLab = !showSessionLab;
      }
    } else {
      konamiIndex = e.key === konamiSequence[0] ? 1 : 0;
    }
  }

  function shouldIgnoreGlobalShortcut(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false;
    if (target.isContentEditable) return true;
    const tagName = target.tagName;
    return tagName === 'INPUT' || tagName === 'TEXTAREA' || tagName === 'SELECT';
  }

  function handleGlobalShortcut(e: KeyboardEvent) {
    if (shouldIgnoreGlobalShortcut(e.target)) return;
    if (e.key === ';') {
      e.preventDefault();
      showActionsPreferences = true;
    }
  }

  onMount(async () => {
    document.addEventListener('keydown', handleKonamiKey);
    document.addEventListener('keydown', handleGlobalShortcut);
    onOpenActionsPreferences = () => {
      showActionsPreferences = true;
    };
    window.addEventListener('staged:open-actions-preferences', onOpenActionsPreferences);

    // Listen for the Help → Health Check… menu item.
    unlistenDoctor = await listen('menu:doctor', () => {
      showDoctor = true;
    });

    // Listen for session status changes globally to handle spinner cleanup
    // This must be at the App level so it works regardless of which view the user is on
    unlistenSessionStatus = await listen<{
      sessionId: string;
      status: string;
    }>('session-status-changed', (event) => {
      const { sessionId, status } = event.payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        // Handle session completion - mark project as unread if user is not viewing it
        projectStateStore.handleSessionComplete(sessionId, navigation.selectedProjectId);
      }
    });

    const t0 = performance.now();
    try {
      await initPreferences();
    } catch (e) {
      console.error('Failed to initialize preferences, rendering with defaults:', e);
      preferences.loaded = true;
    }
    console.debug(`[Staged] preferences ready in ${Math.round(performance.now() - t0)}ms`);

    try {
      storeIncompat = await commands.getStoreStatus();
    } catch (e) {
      storeError = e instanceof Error ? e.message : String(e);
    }

    // Discover available agents in the background.
    await refreshProviders();

    // Check for `sq` CLI in the background (non-blocking).
    refreshSqAvailability();

    // Window was created hidden — show it now that the theme is applied
    await getCurrentWindow().show();
  });

  onDestroy(() => {
    document.removeEventListener('keydown', handleKonamiKey);
    document.removeEventListener('keydown', handleGlobalShortcut);
    if (onOpenActionsPreferences) {
      window.removeEventListener('staged:open-actions-preferences', onOpenActionsPreferences);
      onOpenActionsPreferences = null;
    }
    unlistenDoctor?.();
    unlistenSessionStatus?.();
  });

  async function handleResetStore() {
    resetting = true;
    storeError = null;
    try {
      await commands.confirmResetStore();
      storeIncompat = null;
    } catch (e) {
      storeError = e instanceof Error ? e.message : String(e);
    } finally {
      resetting = false;
    }
  }

  function handleClose() {
    getCurrentWindow().close();
  }
</script>

{#if preferences.loaded}
  {#if storeIncompat && storeIncompat.kind === 'needs_reset'}
    <main class="reset-shell">
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Required</h1>
            <span class="version-badge new">v{storeIncompat.appVersion}</span>
          </div>
          <p>
            Staged beta updates can require backwards-incompatible changes. The info stored by
            Staged (session history, notes) will be cleared, but your
            <strong>git repos and branches are not affected</strong>.
          </p>
          <div class="update-footer">
            <p class="version-hint">
              Not ready? Install <code>v{storeIncompat.dbAppVersion}</code> instead.
            </p>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
              <button class="reset-button" onclick={handleResetStore} disabled={resetting}>
                {resetting ? 'Resetting…' : 'Reset & Update'}
              </button>
            </div>
          </div>
        </div>
      </div>
    </main>
  {:else if storeIncompat && storeIncompat.kind === 'too_new'}
    <main class="reset-shell">
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Staged</h1>
            <span class="version-badge new">v{storeIncompat.dbAppVersion}</span>
          </div>
          <p>
            This database was last used by a newer version of Staged. Please install
            <strong>v{storeIncompat.dbAppVersion}</strong> or newer to continue.
          </p>
          <div class="update-footer">
            <div></div>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
            </div>
          </div>
        </div>
      </div>
    </main>
  {:else}
    <main>
      <TopBar />
      <div class="content">
        {#if storeError}
          <div class="error-state">
            <p>{storeError}</p>
          </div>
        {:else if navigation.selectedProjectId}
          <ProjectHome selectedProjectId={navigation.selectedProjectId} />
        {:else}
          <ProjectsList />
        {/if}
      </div>
    </main>
  {/if}

  {#if showSessionLab}
    <SessionLauncher onClose={() => (showSessionLab = false)} />
  {/if}

  {#if showDoctor}
    <DoctorModal onClose={() => (showDoctor = false)} />
  {/if}

  {#if showActionsPreferences}
    <ActionsPreferencesModal onClose={() => (showActionsPreferences = false)} />
  {/if}

  <ToastHost />
{/if}

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
    background-color: var(--bg-chrome);
    color: var(--text-primary);
  }

  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
    background-color: var(--bg-chrome);
  }

  .content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .reset-shell {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
    color: var(--ui-danger);
  }

  .update-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
  }

  .update-card {
    width: 460px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .update-header {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .update-title {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: -0.03em;
  }

  .version-badge.new {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 4px;
    background-color: rgba(63, 185, 80, 0.12);
    color: var(--ui-accent);
  }

  .update-card > p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.6;
  }

  .update-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .version-hint {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .version-hint code {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    padding: 1px 5px;
    background-color: var(--bg-elevated);
    border-radius: 3px;
    color: var(--text-muted);
  }

  .update-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }

  .close-button {
    padding: 7px 16px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .close-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .reset-button {
    padding: 7px 16px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 8px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 600;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .reset-button:hover {
    background-color: var(--ui-accent-hover);
  }

  .reset-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
