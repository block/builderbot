<!--
  App.svelte — Root shell for Staged

  Initializes preferences (which loads the syntax theme and applies
  adaptive CSS variables), then renders the top bar and main content.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getWindowLabel, isTauri, getWindowSync, type UnlistenFn } from './lib/transport';
  import * as commands from './lib/api/commands';
  import TopBar from './lib/features/layout/TopBar.svelte';
  import ProjectHome from './lib/features/projects/ProjectHome.svelte';
  import ProjectsList from './lib/features/projects/ProjectsList.svelte';
  import ProjectsSidebar from './lib/features/projects/ProjectsSidebar.svelte';
  import ProjectDeleteDialog from './lib/features/projects/ProjectDeleteDialog.svelte';
  import ReposListView from './lib/features/projects/ReposListView.svelte';
  import SessionLauncher from './lib/features/sessions/SessionLauncher.svelte';
  import SettingsPage from './lib/features/settings/SettingsPage.svelte';
  import DiffModal from './lib/features/diff/DiffModal.svelte';
  import ReferenceModalHost from './lib/features/references/ReferenceModalHost.svelte';
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
    closeDiffRouteIfCurrent,
    openSettings,
    popDetailRoute,
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
  import { projectsDataStore } from './lib/stores/projectsData.svelte';
  import { projectRunActionsStore } from './lib/stores/projectRunActions.svelte';
  import { initBloxEnv } from './lib/stores/bloxEnv.svelte';
  import { listenForSessionStatus } from './lib/listeners/sessionStatusListener';
  import { listenForCacheInvalidation } from './lib/listeners/cacheInvalidationListener';
  import { listenForPageLifecycle } from './lib/listeners/pageLifecycleListener';
  import { listenForAcpToolsReconciled } from './lib/listeners/acpToolsListener';
  import { listenForMenuEvents } from './lib/listeners/menuListener';
  import { darkMode } from './lib/stores/isDark.svelte';
  import * as prPollingService from './lib/services/prPollingService';
  import type { StoreIncompatibility } from './lib/types';

  const updaterEnabled = import.meta.env.VITE_UPDATER_ENABLED === 'true';
  const updaterCheckIntervalMs = 15 * 60 * 1000;

  let showSessionLab = $state(false);
  let unlistenMenu: UnlistenFn | undefined;
  let unlistenSessionStatus: UnlistenFn | undefined;
  let unlistenCacheInvalidation: UnlistenFn | undefined;
  let unlistenPageLifecycle: (() => void) | undefined;
  let unlistenAcpToolsReconciled: UnlistenFn | undefined;
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
        const { data: branches } = await commands.listBranchesForProject(projectId);
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

  function requestNewWindow() {
    // The new window inherits this window's selected project, mirroring the
    // macOS menu path in menuListener.ts.
    void commands.newWindow(navigation.selectedProjectId ?? null).catch((e) => {
      console.error('Failed to open new window:', e);
    });
  }

  function navigateBack(): boolean {
    if (!navigation.canGoBack) return false;
    popDetailRoute();
    return true;
  }

  function isTextInputActive(): boolean {
    const target = document.activeElement;
    if (!(target instanceof HTMLElement)) return false;
    if (target.isContentEditable) return true;
    return (
      target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT'
    );
  }

  function requestDeleteCurrentProject(): boolean {
    if (isTextInputActive()) return false;
    if (navigation.currentRoute.kind !== 'project') return false;

    const event = new CustomEvent('staged:delete-current-project', { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
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
    const windowLabel = getWindowLabel();
    void logUpdater(
      `[updater] gate check: enabled=${updaterEnabled} dev=${import.meta.env.DEV} isTauriApp=${isTauriApp} window=${windowLabel}`
    );

    // Update prompts and relaunches are app-wide. Letting every webview run
    // this loop produces duplicate prompts and competing installs.
    if (!updaterEnabled || import.meta.env.DEV || !isTauriApp || windowLabel !== 'main') {
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

    // Web resume accelerator: the restored project id is available synchronously
    // (navigation seeds it from localStorage at module load). Warm that project's
    // branch timelines in parallel right away so cards find data ready, rather
    // than each doing its own serialized IndexedDB read after ProjectHome mounts.
    if (navigation.selectedProjectId) {
      void commands.warmProjectTimelines(navigation.selectedProjectId);
    }

    // App-menu items, all nine registered and torn down as one. See
    // menuListener.ts for why they're window-scoped.
    unlistenMenu = listenForMenuEvents();

    // Global session-status listener — must live at App level so it works
    // regardless of which view the user is on. See sessionStatusListener.ts.
    unlistenSessionStatus = listenForSessionStatus();
    unlistenCacheInvalidation = listenForCacheInvalidation();
    unlistenPageLifecycle = listenForPageLifecycle();
    // Refresh provider discovery (and any loaded doctor report) once the
    // backend finishes installing/upgrading the managed ACP bridges.
    unlistenAcpToolsReconciled = listenForAcpToolsReconciled();
    // Keep the shared project-list cache fresh for the app's lifetime — the
    // store dedupes, so starting before any view consumes it is safe.
    projectsDataStore.startListeners();
    // Run-action state feeds the shared project filters (Running chip,
    // filtered lists), which every route renders via the sidebar or the
    // landing grid — so its listeners are app-lifetime too. A view-scoped
    // lifecycle wiped the state on the repos route, where neither view that
    // used to own it is mounted.
    projectRunActionsStore.startListening();

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
      // New Window is Tauri-only (the web server rejects `new_window`). On
      // macOS the native File ▸ New Window accelerator (⇧⌘N) consumes the
      // keydown before the webview sees it and routes through the menu
      // listener instead, so this binding is effectively for Windows/Linux,
      // where the macOS-only menu — previously the sole caller of
      // `newWindow` — doesn't exist.
      ...(isTauri
        ? [
            {
              id: 'app-new-window',
              description: 'New window',
              category: 'app' as const,
              keys: ['n'],
              modifiers: { meta: true, shift: true },
              handler: requestNewWindow,
            },
          ]
        : []),
      {
        id: 'app-go-back',
        description: 'Back',
        category: 'app',
        keys: ['ArrowLeft'],
        modifiers: { meta: true },
        handler: navigateBack,
      },
      {
        id: 'app-delete-project',
        description: 'Remove current project',
        category: 'app',
        keys: ['Backspace', 'Delete'],
        modifiers: { meta: true },
        handler: requestDeleteCurrentProject,
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
    unlistenMenu?.();
    unlistenSessionStatus?.();
    unlistenCacheInvalidation?.();
    unlistenPageLifecycle?.();
    unlistenAcpToolsReconciled?.();
    projectsDataStore.stopListeners();
    projectRunActionsStore.stopListening();
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
        {:else if navigation.currentRoute.kind === 'diff'}
          {@const diffRoute = navigation.currentRoute}
          <DiffModal
            branchId={diffRoute.branchId}
            projectId={diffRoute.projectId}
            commitSha={diffRoute.commitSha}
            scope={diffRoute.scope}
            reviewId={diffRoute.reviewId}
            beforeLabel={diffRoute.beforeLabel}
            afterLabel={diffRoute.afterLabel}
            readonly={diffRoute.readonly}
            commits={diffRoute.commits}
            baseBranchLabel={diffRoute.baseBranchLabel}
            branchLabel={diffRoute.branchLabel}
            projectName={diffRoute.projectName}
            githubRepo={diffRoute.githubRepo}
            subpath={diffRoute.subpath}
            onClose={() => closeDiffRouteIfCurrent(diffRoute)}
          />
        {:else}
          <div class="workspace">
            <!-- Hoisted out of ProjectHome so it survives project↔repos
                 transitions; hidden on the landing page. -->
            {#if navigation.selectedProjectId || navigation.showReposList}
              <ProjectsSidebar />
            {/if}
            {#if navigation.showReposList}
              <ReposListView />
            {:else if navigation.selectedProjectId}
              <ProjectHome selectedProjectId={navigation.selectedProjectId} />
            {:else}
              <ProjectsList />
            {/if}
          </div>
        {/if}
      </div>
    </main>
  {/if}

  {#if showSessionLab}
    <SessionLauncher onClose={() => (showSessionLab = false)} />
  {/if}

  <!-- Shared remove-project confirmation — serves every route's delete entry
       point (sidebar, landing grid, ProjectHome top bar/shortcut). -->
  <ProjectDeleteDialog />

  <ReferenceModalHost />
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
  }

  /* Sidebar + route view side by side (mirrors ProjectHome's old wrapper). */
  .workspace {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    background-color: var(--bg-chrome);
    overflow: hidden;
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
