<!--
  App.svelte — Root shell for Staged

  Initializes preferences (which loads the syntax theme and applies
  adaptive CSS variables), then renders the top bar, sidebar, and main content.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import TopBar from './lib/TopBar.svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import ProjectsList from './lib/features/projects/ProjectsList.svelte';
  import ProjectHome from './lib/features/projects/ProjectHome.svelte';
  import SessionLauncher from './lib/features/sessions/SessionLauncher.svelte';
  import AgentSetupModal from './lib/features/agents/AgentSetupModal.svelte';
  import {
    preferences,
    initPreferences,
    toggleSidebar,
  } from './lib/features/settings/preferences.svelte';
  import { agentState, refreshProviders } from './lib/features/agents/agent.svelte';
  import { refreshSqAvailability } from './lib/features/settings/sq.svelte';
  import { navigation } from './lib/navigation.svelte';

  let showSessionLab = $state(false);
  let showAgentSetup = $state(false);

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

  function handleGlobalKeydown(e: KeyboardEvent) {
    handleKonamiKey(e);

    // Cmd+B toggles the sidebar
    if (e.metaKey && e.key === 'b') {
      const target = e.target as HTMLElement;
      const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
      if (!isInput) {
        e.preventDefault();
        toggleSidebar();
      }
    }
  }

  onMount(async () => {
    document.addEventListener('keydown', handleGlobalKeydown);
    const t0 = performance.now();
    try {
      await initPreferences();
    } catch (e) {
      console.error('Failed to initialize preferences, rendering with defaults:', e);
      preferences.loaded = true;
    }
    console.debug(`[Staged] preferences ready in ${Math.round(performance.now() - t0)}ms`);

    // Discover available agents. We await so we know whether to show
    // the setup modal before revealing the window.
    await refreshProviders();

    // Check for `sq` CLI in the background (non-blocking).
    refreshSqAvailability();

    // Show the setup modal only when no agents are installed at all.
    if (agentState.providers.length === 0) {
      showAgentSetup = true;
    }

    // Window was created hidden — show it now that the theme is applied
    await getCurrentWindow().show();
  });

  onDestroy(() => {
    document.removeEventListener('keydown', handleGlobalKeydown);
  });
</script>

{#if preferences.loaded}
  <main>
    <TopBar onToggleSidebar={toggleSidebar} sidebarOpen={preferences.sidebarOpen} />
    <div class="body">
      {#if preferences.sidebarOpen}
        <Sidebar />
      {/if}
      <div class="content">
        {#if navigation.selectedProjectId}
          <ProjectHome projectId={navigation.selectedProjectId} />
        {:else}
          <ProjectsList />
        {/if}
      </div>
    </div>
  </main>

  {#if showAgentSetup}
    <AgentSetupModal onClose={() => (showAgentSetup = false)} />
  {/if}

  {#if showSessionLab}
    <SessionLauncher onClose={() => (showSessionLab = false)} />
  {/if}
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

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
</style>
