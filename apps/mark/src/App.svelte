<!--
  App.svelte — Root shell for Mark

  Initializes preferences (which loads the syntax theme and applies
  adaptive CSS variables), then renders the top bar and main content.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import * as commands from './lib/api/commands';
  import TopBar from './lib/features/layout/TopBar.svelte';
  import ProjectHome from './lib/features/projects/ProjectHome.svelte';
  import ProjectsList from './lib/features/projects/ProjectsList.svelte';
  import SessionLauncher from './lib/features/sessions/SessionLauncher.svelte';
  import SettingsPage from './lib/features/settings/SettingsPage.svelte';
  import ToastHost from './lib/shared/ToastHost.svelte';
  import {
    preferences,
    initPreferences,
    setAiAgent,
    increaseSize,
    decreaseSize,
    resetSize,
  } from './lib/features/settings/preferences.svelte';
  import { refreshProviders } from './lib/features/agents/agent.svelte';
  import { refreshSqAvailability } from './lib/features/settings/sq.svelte';
  import {
    navigation,
    initNavigation,
    openSettings,
  } from './lib/features/layout/navigation.svelte';
  import {
    initializeShortcutBindings,
    registerShortcuts,
    triggerShortcut,
  } from './lib/features/keyboard/shortcuts';
  import { runSearchShortcut } from './lib/features/keyboard/searchTargets';
  import { projectStateStore } from './lib/stores/projectState.svelte';
  import { prStateStore } from './lib/stores/prState.svelte';
  import { pushStateStore } from './lib/stores/pushState.svelte';
  import { sessionRegistry } from './lib/stores/sessionRegistry.svelte';
  import {
    extractPrUrl,
    extractPrNumber,
    isPushRejectedNonFastForward,
  } from './lib/features/branches/branchCardHelpers';
  import type { StoreIncompatibility } from './lib/types';

  let showSessionLab = $state(false);
  let unlistenSettings: UnlistenFn | undefined;
  let unlistenFind: UnlistenFn | undefined;
  let unlistenFindNext: UnlistenFn | undefined;
  let unlistenFindPrevious: UnlistenFn | undefined;
  let unlistenZoomIn: UnlistenFn | undefined;
  let unlistenZoomOut: UnlistenFn | undefined;
  let unlistenZoomReset: UnlistenFn | undefined;
  let unlistenSessionStatus: UnlistenFn | undefined;
  let unregisterShortcuts: (() => void) | null = null;
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

  function requestNewProject() {
    if (navigation.activeView === 'settings') return;
    window.dispatchEvent(new CustomEvent('mark:new-project'));
  }

  onMount(async () => {
    document.addEventListener('keydown', handleKonamiKey);

    // Listen for the app menu Preferences item.
    unlistenSettings = await listen('menu:settings', () => {
      if (!triggerShortcut('app-open-settings')) openSettings();
    });
    unlistenFind = await listen('menu:find', () => {
      if (!triggerShortcut('search-find')) runSearchShortcut('find');
    });
    unlistenFindNext = await listen('menu:find-next', () => {
      if (!triggerShortcut('search-find-next')) runSearchShortcut('next');
    });
    unlistenFindPrevious = await listen('menu:find-previous', () => {
      if (!triggerShortcut('search-find-previous')) runSearchShortcut('previous');
    });
    unlistenZoomIn = await listen('menu:zoom-in', () => {
      if (!triggerShortcut('view-increase-size')) increaseSize();
    });
    unlistenZoomOut = await listen('menu:zoom-out', () => {
      if (!triggerShortcut('view-decrease-size')) decreaseSize();
    });
    unlistenZoomReset = await listen('menu:zoom-reset', () => {
      if (!triggerShortcut('view-reset-size')) resetSize();
    });

    // Listen for session status changes globally to handle spinner cleanup
    // This must be at the App level so it works regardless of which view the user is on
    //
    // Session completion handler updates THREE independent state stores:
    // 1. projectState: Aggregate view of all sessions in a project (for project tiles)
    // 2. prState: Branch-specific PR creation workflow state (for PR buttons)
    // 3. pushState: Branch-specific push workflow state (for push operations)
    //
    // Session lookups are delegated to the unified sessionRegistry for consistency
    unlistenSessionStatus = await listen<{
      sessionId: string;
      status: string;
      branchId?: string;
      projectId?: string;
      sessionType?: string;
    }>('session-status-changed', async (event) => {
      const {
        sessionId,
        status,
        branchId: eventBranchId,
        projectId: eventProjectId,
        sessionType,
      } = event.payload;

      // MCP-initiated repo session just started — register it so the project
      // spinner shows and the completion handler can clean it up correctly.
      if (status === 'running' && eventProjectId) {
        sessionRegistry.register(
          sessionId,
          eventProjectId,
          (sessionType as import('./lib/stores/sessionRegistry.svelte').SessionType) ?? 'other',
          eventBranchId
        );
        projectStateStore.addRunningSession(eventProjectId, sessionId);
        return;
      }

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

        // Handle push-specific completion logic
        if (sessionType === 'push' && branchId) {
          if (status === 'completed') {
            try {
              // Check session messages for the non-fast-forward rejection marker
              const messages = await commands.getSessionMessages(sessionId);
              if (isPushRejectedNonFastForward(messages)) {
                // The agent stopped because the remote would lose commits.
                // Go to error state — clicking the button will open the force push dialog.
                pushStateStore.setPushError(branchId, '', true); // rejectedNonFastForward=true
              } else {
                // Push completed successfully — clear stale PR status (checks,
                // mergeable, etc.) before marking done so the UI doesn't briefly
                // flash outdated indicators like "Has conflicts".
                try {
                  await commands.clearBranchPrStatus(branchId);
                } catch (e) {
                  console.warn('[Mark] Failed to clear PR status after push:', e);
                }
                pushStateStore.setPushDone(branchId);
                // Reset to idle after a brief moment so the button returns to "View PR"
                setTimeout(() => {
                  pushStateStore.clearPushState(branchId);
                }, 1_500);
              }
            } catch (e) {
              // Failed to get session messages - treat as error
              pushStateStore.setPushError(branchId, e instanceof Error ? e.message : String(e));
            }
          } else {
            // Session errored or was cancelled
            pushStateStore.setPushError(
              branchId,
              `Push session ${status === 'error' ? 'failed' : 'was cancelled'}.`
            );
          }
          // Clear push state's session tracking (does NOT unregister from registry)
          pushStateStore.clearSessionTracking(branchId);
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
      await initializeShortcutBindings();
    } catch (e) {
      console.warn('Failed to initialize custom keyboard bindings:', e);
    }

    unregisterShortcuts = registerShortcuts([
      {
        id: 'app-open-settings',
        description: 'Open settings',
        category: 'app',
        keys: [','],
        modifiers: { meta: true },
        allowInInputs: true,
        handler: () => openSettings(),
      },
      {
        id: 'app-new-project',
        description: 'New project',
        category: 'app',
        keys: ['n'],
        modifiers: { meta: true },
        handler: requestNewProject,
      },
      {
        id: 'search-find',
        description: 'Find in open note/session',
        category: 'search',
        keys: ['f'],
        modifiers: { meta: true },
        allowInInputs: true,
        handler: () => {
          runSearchShortcut('find');
        },
      },
      {
        id: 'search-find-next',
        description: 'Find next match',
        category: 'search',
        keys: ['g'],
        modifiers: { meta: true },
        allowInInputs: true,
        handler: () => {
          runSearchShortcut('next');
        },
      },
      {
        id: 'search-find-previous',
        description: 'Find previous match',
        category: 'search',
        keys: ['g'],
        modifiers: { meta: true, shift: true },
        allowInInputs: true,
        handler: () => {
          runSearchShortcut('previous');
        },
      },
      {
        id: 'view-increase-size',
        description: 'Increase text size',
        category: 'view',
        keys: ['=', '+'],
        modifiers: { meta: true },
        allowInInputs: true,
        handler: increaseSize,
      },
      {
        id: 'view-decrease-size',
        description: 'Decrease text size',
        category: 'view',
        keys: ['-'],
        modifiers: { meta: true },
        allowInInputs: true,
        handler: decreaseSize,
      },
      {
        id: 'view-reset-size',
        description: 'Reset text size',
        category: 'view',
        keys: ['0'],
        modifiers: { meta: true },
        allowInInputs: true,
        handler: resetSize,
      },
    ]);

    try {
      storeIncompat = await commands.getStoreStatus();
    } catch (e) {
      storeError = e instanceof Error ? e.message : String(e);
    }

    // Discover available agents in the background.
    const providers = await refreshProviders();

    // First launch: default to the first discovered agent so commit/note actions
    // don't run with an empty provider.
    if (preferences.recentAgents.length === 0 && providers.length > 0) {
      setAiAgent(providers[0].id);
    }

    // Check for `sq` CLI in the background (non-blocking).
    refreshSqAvailability();

    // Window was created hidden — show it now that the theme is applied
    await getCurrentWindow().show();
  });

  onDestroy(() => {
    document.removeEventListener('keydown', handleKonamiKey);
    unregisterShortcuts?.();
    unlistenSettings?.();
    unlistenFind?.();
    unlistenFindNext?.();
    unlistenFindPrevious?.();
    unlistenZoomIn?.();
    unlistenZoomOut?.();
    unlistenZoomReset?.();
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
