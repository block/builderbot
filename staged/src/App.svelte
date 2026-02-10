<!--
  App.svelte — Root shell for Staged

  Initializes preferences (which loads the syntax theme and applies
  adaptive CSS variables), then renders the top bar and main content.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import TopBar from './lib/TopBar.svelte';
  import ProjectHome from './lib/ProjectHome.svelte';
  import SessionLauncher from './lib/SessionLauncher.svelte';
  import AgentSetupModal from './lib/AgentSetupModal.svelte';
  import { preferences, initPreferences } from './lib/stores/preferences.svelte';
  import { agentState, refreshProviders } from './lib/stores/agent.svelte';

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

  onMount(async () => {
    document.addEventListener('keydown', handleKonamiKey);
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

    // Show the setup modal only when no agents are installed at all.
    if (agentState.providers.length === 0) {
      showAgentSetup = true;
    }

    // Window was created hidden — show it now that the theme is applied
    await getCurrentWindow().show();
  });

  onDestroy(() => {
    document.removeEventListener('keydown', handleKonamiKey);
  });
</script>

{#if preferences.loaded}
  <main>
    <TopBar />
    <div class="content">
      <ProjectHome />
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

  .content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
