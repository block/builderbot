<!--
  App.svelte — Root shell for Mark

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
  import SettingsPage from './lib/features/settings/SettingsPage.svelte';
  import ToastHost from './lib/shared/ToastHost.svelte';
  import { preferences, initPreferences } from './lib/features/settings/preferences.svelte';
  import { refreshProviders } from './lib/features/agents/agent.svelte';
  import { refreshSqAvailability } from './lib/features/settings/sq.svelte';
  import { navigation, initNavigation, openSettings } from './lib/navigation.svelte';
  import { projectStateStore } from './lib/stores/projectState.svelte';
  import { prStateStore } from './lib/stores/prState.svelte';
  import { sessionRegistry } from './lib/stores/sessionRegistry.svelte';
  import { extractPrUrl, extractPrNumber } from './lib/features/branches/branchCardHelpers';
  import type { StoreIncompatibility } from './lib/types';

  let showSessionLab = $state(false);
  let unlistenDoctor: UnlistenFn | undefined;
  let unlistenSettings: UnlistenFn | undefined;
  let unlistenSessionStatus: UnlistenFn | undefined;
  let storeIncompat = $state<StoreIncompatibility | null>(null);
  let resetting = $state(false);
  let storeError = $state<string | null>(null);

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
    if ((e.metaKey || e.ctrlKey) && e.key === ',') {
      e.preventDefault();
      openSettings();
    }
  }

  onMount(async () => {
    document.addEventListener('keydown', handleKonamiKey);
    document.addEventListener('keydown', handleGlobalShortcut);

    // Listen for the Help → Health Check... menu item.
    unlistenDoctor = await listen('menu:doctor', () => {
      openSettings('doctor');
    });
    // Listen for the app menu Preferences item.
    unlistenSettings = await listen('menu:settings', () => {
      openSettings();
    });

    // Listen for session status changes globally to handle spinner cleanup
    // This must be at the App level so it works regardless of which view the user is on
    //
    // Session completion handler updates TWO independent state stores:
    // 1. projectState: Aggregate view of all sessions in a project (for project tiles)
    // 2. prState: Branch-specific PR creation workflow state (for PR buttons)
    //
    // Session lookups are delegated to the unified sessionRegistry for consistency
    unlistenSessionStatus = await listen<{
      sessionId: string;
      status: string;
    }>('session-status-changed', async (event) => {
      const { sessionId, status } = event.payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        // Get session metadata from the unified registry
        const sessionProjectId = sessionRegistry.getProjectId(sessionId);
        const sessionType = sessionRegistry.getType(sessionId);
        const branchId = sessionRegistry.getBranchId(sessionId);
        const currentProjectId = navigation.selectedProjectId;
        if (!sessionProjectId && !sessionType && !branchId) {
          console.warn('Received completion event for unknown session ID', { sessionId, status });
        }

        // Mark project as unread if:
        // 1. We know which project the session belonged to AND
        // 2. The user is currently viewing a different project
        if (sessionProjectId && currentProjectId !== sessionProjectId) {
          projectStateStore.markAsUnread(sessionProjectId);
        }

        // Always remove the running session from its project
        if (sessionProjectId) {
          projectStateStore.removeRunningSession(sessionProjectId, sessionId);
        }

        // Handle PR-specific completion logic
        if (sessionType === 'pr' && branchId) {
          if (status === 'completed') {
            try {
              // Fetch session messages to find the PR URL
              const messages = await commands.getSessionMessages(sessionId);
              const foundUrl = extractPrUrl(messages);

              if (foundUrl) {
                const prNumber = extractPrNumber(foundUrl);
                if (prNumber) {
                  try {
                    // Save PR number to storage (separate try-catch to handle storage failures)
                    await commands.updateBranchPr(branchId, prNumber);
                  } catch (storageError) {
                    // If storage fails, we still have the PR URL from the session
                    // Log the error but don't fail the PR creation - the PR exists on GitHub
                    console.error('Failed to persist PR number to storage:', storageError);
                  }
                }
                // Set state to created regardless of storage success - the PR was created
                prStateStore.setPrCreated(branchId, foundUrl);
              } else {
                // Session completed but we couldn't find a PR URL
                prStateStore.setPrError(
                  branchId,
                  'PR session completed but no PR URL was found in the output.'
                );
              }
            } catch (e) {
              // Failed to get session messages or extract PR URL
              prStateStore.setPrError(branchId, e instanceof Error ? e.message : String(e));
            }
          } else {
            // Session errored or was cancelled
            prStateStore.setPrError(
              branchId,
              `PR creation session ${status === 'error' ? 'failed' : 'was cancelled'}.`
            );
          }
          // Clear PR state's session tracking (does NOT unregister from registry)
          prStateStore.clearSessionTracking(branchId);
        }

        // Clean up the session from the unified registry (single point of cleanup)
        sessionRegistry.unregister(sessionId);
      }
    });

    const t0 = performance.now();
    try {
      await initPreferences();
    } catch (e) {
      console.error('Failed to initialize preferences, rendering with defaults:', e);
      preferences.loaded = true;
    }
    console.debug(`[Mark] preferences ready in ${Math.round(performance.now() - t0)}ms`);

    // Restore the last viewed project (persistent store is now ready).
    try {
      await initNavigation();
    } catch (e) {
      console.error('Failed to restore last viewed project:', e);
    }

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
    unlistenDoctor?.();
    unlistenSettings?.();
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
            Mark beta updates can require backwards-incompatible changes. The info stored by Mark
            (session history, notes) will be cleared, but your
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
            <h1 class="update-title">Update Mark</h1>
            <span class="version-badge new">v{storeIncompat.dbAppVersion}</span>
          </div>
          <p>
            This database was last used by a newer version of Mark. Please install
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
        {:else if navigation.activeView === 'settings'}
          <SettingsPage />
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
    border-top: 1px solid color-mix(in srgb, var(--border-subtle) 50%, transparent);
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
