<!--
  SplashScreen.svelte - Welcome screen shown when no projects exist

  Displays branded icon, tagline, gradient background, and
  a call-to-action to create the first project.
-->
<script lang="ts">
  import GitTreeAnimation from '../../shared/GitTreeAnimation.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';

  let { onCreateProject }: { onCreateProject: () => void } = $props();
</script>

<div class="splash">
  <div class="splash-glow glow-a"></div>
  <div class="splash-glow glow-b"></div>

  <div class="splash-center">
    <div class="icon-frame">
      <StagedIcon size={52} />
    </div>
    <h2 class="splash-heading">
      Git branch workflows,<br />beautifully <span class="mono accent">staged</span>
    </h2>
  </div>

  <div class="splash-actions">
    <button class="splash-pill" onclick={onCreateProject}> Create your first project </button>
    <span class="splash-hint">or press <kbd>⌘ N</kbd> anytime</span>
  </div>

  <div class="splash-tree">
    <GitTreeAnimation />
  </div>
</div>

<style>
  .splash {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    flex: 1;
    overflow: hidden;
    padding: 48px 24px 0;
  }

  .splash-glow {
    position: absolute;
    border-radius: 50%;
    filter: blur(140px);
    pointer-events: none;
  }

  .glow-a {
    width: 600px;
    height: 600px;
    top: -15%;
    left: -10%;
    background: var(--ui-accent);
    opacity: 0.06;
  }

  .glow-b {
    width: 500px;
    height: 500px;
    bottom: -10%;
    right: -10%;
    background: var(--review-color, #a78bfa);
    opacity: 0.07;
  }

  .splash-center {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 28px;
    z-index: 1;
  }

  .icon-frame {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 104px;
    height: 104px;
    border-radius: 26px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    box-shadow:
      0 8px 32px rgba(0, 0, 0, 0.2),
      0 0 0 1px var(--border-subtle);
  }

  .splash-heading {
    font-size: 22px;
    font-weight: 400;
    color: var(--text-primary);
    text-align: center;
    line-height: 1.5;
    margin: 0;
    letter-spacing: -0.01em;
  }

  .splash-heading .mono {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    letter-spacing: -0.02em;
  }

  .splash-heading .accent {
    color: var(--ui-accent);
  }

  .splash-actions {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding-bottom: 32px;
    z-index: 1;
  }

  .splash-pill {
    padding: 12px 36px;
    border-radius: 999px;
    border: 1px solid transparent;
    background: var(--bg-elevated);
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.2s ease,
      border-color 0.2s ease,
      transform 0.15s ease;
  }

  .splash-pill:hover {
    background: var(--bg-hover);
    border-color: var(--border-muted);
    transform: translateY(-1px);
  }

  .splash-pill:active {
    transform: translateY(0);
  }

  .splash-hint {
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  .splash-hint kbd {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    padding: 2px 6px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-muted);
  }

  .splash-tree {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    opacity: 0.25;
    pointer-events: none;
  }

  .splash-tree :global(.animation-wrapper) {
    width: 100%;
  }
</style>
