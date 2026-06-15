<!--
  App.svelte — Root shell for Staged

  Initializes preferences (which loads the syntax theme and applies
  adaptive CSS variables), then renders the top bar and main content.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { isTauri, listenToEvent, getWindowSync, type UnlistenFn } from './lib/transport';
  import * as commands from './lib/api/commands';
  import TopBar from './lib/features/layout/TopBar.svelte';
  import ProjectHome from './lib/features/projects/ProjectHome.svelte';
  import ProjectsList from './lib/features/projects/ProjectsList.svelte';
  import ReposListView from './lib/features/projects/ReposListView.svelte';
  import SessionLauncher from './lib/features/sessions/SessionLauncher.svelte';
  import SettingsPage from './lib/features/settings/SettingsPage.svelte';
  import { Toaster } from '$lib/components/ui/sonner';
  import { Button } from '$lib/components/ui/button';
  import {
    preferences,
    initPreferences,
    setAiAgent,
    increaseSize,
    decreaseSize,
    resetSize,
  } from './lib/features/settings/preferences.svelte';
  import { refreshProviders } from './lib/features/agents/agent.svelte';
  import { ensureSqAvailabilityLoaded } from './lib/features/settings/sq.svelte';
  import {
    navigation,
    initNavigation,
    openSettings,
    selectPreviousProject,
    selectNextProject,
  } from './lib/features/layout/navigation.svelte';
  import {
    initializeShortcutBindings,
    registerShortcuts,
    triggerShortcut,
  } from './lib/features/keyboard/shortcuts';
  import { runSearchShortcut } from './lib/features/keyboard/searchTargets';
  import { projectStateStore } from './lib/stores/projectState.svelte';
  import { initBloxEnv } from './lib/stores/bloxEnv.svelte';
  import { listenForSessionStatus } from './lib/listeners/sessionStatusListener';
  import { darkMode } from './lib/stores/isDark.svelte';
  import * as prPollingService from './lib/services/prPollingService';
  import { reposUiEnabled } from './lib/featureFlags';
  import type { StoreIncompatibility } from './lib/types';

  const updaterEnabled = import.meta.env.VITE_UPDATER_ENABLED === 'true';
  const updaterCheckIntervalMs = 15 * 60 * 1000;

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
  let stopUpdaterLoop: (() => void) | null = null;
  let storeIncompat = $state<StoreIncompatibility | null>(null);
  let resetting = $state(false);
  let storeError = $state<string | null>(null);

  // =========================================================================
  // App-wide PR polling — the backend scheduler owns cadence/concurrency and
  // derives the project list from the DB. The frontend only forwards the
  // selected project as an interest hint (focus + lifecycle wiring is in
  // prPollingService.init(), called from onMount).
  // =========================================================================
  $effect(() => {
    prPollingService.setSelectedProject(navigation.selectedProjectId);
  });

  // Refresh git state (TTL-gated) for the selected project's branches when the
  // window regains focus, so the user sees a fresh-enough view on return.
  $effect(() => {
    if (!isTauri) return;
    const handler = async () => {
      const projectId = navigation.selectedProjectId;
      if (!projectId) return;
      try {
        const branches = await commands.listBranchesForProject(projectId);
        await Promise.allSettled(branches.map((b) => commands.refreshBranchGitState(b.id)));
      } catch {
        // best-effort refresh; ignore failures
      }
    };
    window.addEventListener('focus', handler);
    return () => window.removeEventListener('focus', handler);
  });

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
    window.dispatchEvent(new CustomEvent('staged:new-project'));
  }

  async function logUpdater(message: string) {
    console.warn(message);
    try {
      const { warn } = await import('@tauri-apps/plugin-log');
      await warn(message);
    } catch {
      // log plugin is optional during local web-only development
    }
  }

  function startUpdaterLoop(): () => void {
    const isTauriApp = isTauri;
    void logUpdater(
      `[updater] gate check: enabled=${updaterEnabled} dev=${import.meta.env.DEV} isTauriApp=${isTauriApp}`
    );

    if (!updaterEnabled || import.meta.env.DEV || !isTauriApp) {
      void logUpdater('[updater] skipped because a gate condition was not met');
      return () => {};
    }

    let cancelled = false;
    let updaterRunInFlight = false;
    let updaterRunQueued = false;
    let lastPromptedUpdateVersion: string | null = null;

    const runUpdater = async () => {
      if (updaterRunInFlight) {
        updaterRunQueued = true;
        await logUpdater('[updater] check skipped because a previous run is still active');
        return;
      }

      updaterRunInFlight = true;
      try {
        await logUpdater('[updater] checking for updates');
        const [{ check }, { relaunch }] = await Promise.all([
          import('@tauri-apps/plugin-updater'),
          import('@tauri-apps/plugin-process'),
        ]);
        const updatePromise = check();
        const timeoutPromise = new Promise<null>((resolve) =>
          setTimeout(() => resolve(null), 15_000)
        );
        const update = await Promise.race([updatePromise, timeoutPromise]);

        if (!update || cancelled) {
          await logUpdater('[updater] no update available, cancelled, or timed out');
          return;
        }

        if (lastPromptedUpdateVersion === update.version) {
          await logUpdater(`[updater] update ${update.version} already prompted; skipping`);
          return;
        }

        lastPromptedUpdateVersion = update.version;
        await logUpdater(`[updater] update found: ${update.version}`);

        const { confirm } = await import('@tauri-apps/plugin-dialog');
        const shouldInstall = await confirm(
          `Staged ${update.version} is available. Install now? The app will restart once finished.`,
          { title: 'Update Available', kind: 'info' }
        );
        if (!shouldInstall || cancelled) {
          await logUpdater(`[updater] install deferred for ${update.version}`);
          return;
        }

        await logUpdater('[updater] downloading and installing update');
        await update.downloadAndInstall();
        if (cancelled) return;

        await relaunch();
      } catch (error) {
        await logUpdater(`[updater] check failed: ${error}`);
      } finally {
        updaterRunInFlight = false;
        if (!cancelled && updaterRunQueued) {
          updaterRunQueued = false;
          void runUpdater();
        }
      }
    };

    void runUpdater();
    const interval = setInterval(() => {
      void runUpdater();
    }, updaterCheckIntervalMs);

    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }

  onMount(async () => {
    darkMode.init();
    // Wire up PR-polling interest hints (window focus + backend lifecycle events).
    prPollingService.init();
    document.addEventListener('keydown', handleKonamiKey);

    // Listen for the app menu Preferences item.
    unlistenSettings = listenToEvent('menu:settings', () => {
      if (!triggerShortcut('app-open-settings')) openSettings();
    });
    unlistenFind = listenToEvent('menu:find', () => {
      if (!triggerShortcut('search-find')) runSearchShortcut('find');
    });
    unlistenFindNext = listenToEvent('menu:find-next', () => {
      if (!triggerShortcut('search-find-next')) runSearchShortcut('next');
    });
    unlistenFindPrevious = listenToEvent('menu:find-previous', () => {
      if (!triggerShortcut('search-find-previous')) runSearchShortcut('previous');
    });
    unlistenZoomIn = listenToEvent('menu:zoom-in', () => {
      if (!triggerShortcut('view-increase-size')) increaseSize();
    });
    unlistenZoomOut = listenToEvent('menu:zoom-out', () => {
      if (!triggerShortcut('view-decrease-size')) decreaseSize();
    });
    unlistenZoomReset = listenToEvent('menu:zoom-reset', () => {
      if (!triggerShortcut('view-reset-size')) resetSize();
    });

    // Global session-status listener — must live at App level so it works
    // regardless of which view the user is on. See sessionStatusListener.ts.
    unlistenSessionStatus = listenForSessionStatus();

    try {
      await initPreferences();
    } catch (e) {
      console.error('Failed to initialize preferences, rendering with defaults:', e);
      preferences.loaded = true;
    }

    // Restore the last viewed project (persistent store is now ready).
    try {
      await initNavigation();
    } catch (e) {
      console.error('Failed to restore last viewed project:', e);
    }

    // Cache the BLOX_ENV value once at startup (process-level, never changes).
    initBloxEnv();

    // Restore persisted unread state and sync the dock badge.
    try {
      await projectStateStore.initFromStore();
      // If we restored into a project that was unread, mark it as read now
      if (
        navigation.selectedProjectId &&
        projectStateStore.isUnread(navigation.selectedProjectId)
      ) {
        projectStateStore.markAsRead(navigation.selectedProjectId);
      }
    } catch (e) {
      console.error('Failed to restore unread state:', e);
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
      {
        id: 'app-previous-project',
        description: 'Previous project',
        category: 'app',
        keys: ['ArrowUp'],
        modifiers: { meta: true },
        handler: () => selectPreviousProject(),
      },
      {
        id: 'app-next-project',
        description: 'Next project',
        category: 'app',
        keys: ['ArrowDown'],
        modifiers: { meta: true },
        handler: () => selectNextProject(),
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

    // Check for `sq` CLI in the background if preferences did not need it.
    ensureSqAvailabilityLoaded();

    // Window was created hidden — show it now that the theme is applied
    await getWindowSync().show();
    stopUpdaterLoop = startUpdaterLoop();
  });

  onDestroy(() => {
    prPollingService.dispose();
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
    stopUpdaterLoop?.();
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
    getWindowSync().close();
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
              <Button variant="ghost" size="sm" onclick={handleClose}>Close</Button>
              <Button variant="outline" size="sm" onclick={handleResetStore} disabled={resetting}>
                {resetting ? 'Resetting…' : 'Reset & Update'}
              </Button>
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
              <Button variant="ghost" size="sm" onclick={handleClose}>Close</Button>
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
        {:else if reposUiEnabled && navigation.showReposList}
          <ReposListView />
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

  <Toaster position="bottom-right" visibleToasts={4} duration={8000} closeButton expand />
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
</style>
