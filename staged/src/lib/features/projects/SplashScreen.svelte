<!--
  SplashScreen.svelte - Welcome screen shown when no projects exist

  Displays branded icon, rotating tagline, gradient background, and
  a call-to-action to create the first project. When the user clicks
  "Create your first project" (or the parent signals via requestOpen),
  the form slides in below the headline.
-->
<script lang="ts">
  import { X } from 'lucide-svelte';
  import type { Project } from '../../types';
  import GitTreeAnimation from '../../shared/GitTreeAnimation.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';
  import NewProjectForm from './NewProjectForm.svelte';

  interface Props {
    onCreated: (project: Project) => void;
    requestOpen?: boolean;
    onFormOpenChange?: (open: boolean) => void;
  }

  let { onCreated, requestOpen = false, onFormOpenChange }: Props = $props();

  let showForm = $state(false);

  const phrases = [
    'AI coding sessions,',
    'From prompt to pull request,',
    'Direct what AI builds,',
    'Review what agents build,',
    'AI-driven development,',
    'Orchestrate AI agents,',
    'AI writes the code,',
    'Prompt, review, ship,',
    'Code at the speed of AI,',
  ];

  let currentIndex = $state(0);
  let transitioning = $state(false);

  $effect(() => {
    if (showForm) return;
    const id = setInterval(() => {
      transitioning = true;
      setTimeout(() => {
        currentIndex = (currentIndex + 1) % phrases.length;
        transitioning = false;
      }, 300);
    }, 5000);
    return () => clearInterval(id);
  });

  $effect(() => {
    if (requestOpen && !showForm) {
      showForm = true;
    }
  });

  function openForm() {
    showForm = true;
    onFormOpenChange?.(true);
  }

  function closeForm() {
    showForm = false;
    onFormOpenChange?.(false);
  }
</script>

<div class="splash">
  <div class="splash-glow glow-a"></div>
  <div class="splash-glow glow-b"></div>

  <div class="splash-center" class:form-open={showForm}>
    <div class="icon-frame" class:collapsed={showForm}>
      <StagedIcon size={52} />
    </div>
    <h2 class="splash-heading">
      <span class="tagline-collapse" class:collapsed={showForm}>
        <span class="tagline-inner">
          <span class="phrase-rotator" class:transitioning>
            {phrases[currentIndex]}
          </span>
        </span>
      </span>
      <span class="staged-line">
        <span class="beautifully-text" class:collapsed={showForm}>beautifully</span><span
          class="mono accent">staged</span
        >
      </span>
    </h2>

    {#if showForm}
      <div class="inline-form">
        <NewProjectForm {onCreated} />
      </div>
    {/if}
  </div>

  {#if !showForm}
    <div class="splash-actions">
      <button class="splash-pill" onclick={openForm}> Create your first project </button>
      <span class="splash-hint">or press <kbd>⌘ N</kbd> anytime</span>
    </div>
    <div class="splash-tree">
      <GitTreeAnimation />
    </div>
  {:else}
    <div class="splash-close">
      <button class="close-circle" onclick={closeForm}>
        <X size={18} />
      </button>
    </div>
  {/if}
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
    transition: gap 0.4s ease;
  }

  .splash-center.form-open {
    gap: 4px;
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
    transform-origin: bottom center;
    transition:
      margin 0.4s ease,
      background-color 0.4s ease,
      border-color 0.4s ease,
      box-shadow 0.4s ease,
      transform 0.4s ease;
  }

  .icon-frame.collapsed {
    margin-top: -50px;
    background: transparent;
    border-color: transparent;
    box-shadow: none;
    transform: scale(0.6);
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

  .tagline-collapse {
    display: grid;
    grid-template-rows: 1fr;
    transition: grid-template-rows 0.4s ease;
  }

  .tagline-collapse.collapsed {
    grid-template-rows: 0fr;
  }

  .tagline-inner {
    overflow: hidden;
    transition: opacity 0.3s ease;
  }

  .tagline-collapse.collapsed .tagline-inner {
    opacity: 0;
  }

  .staged-line {
    display: flex;
    align-items: baseline;
    justify-content: center;
    gap: 6px;
  }

  .beautifully-text {
    display: inline-block;
    max-width: 200px;
    overflow: hidden;
    white-space: nowrap;
    transition:
      max-width 0.4s ease,
      opacity 0.3s ease;
  }

  .beautifully-text.collapsed {
    max-width: 0;
    opacity: 0;
  }

  .phrase-rotator {
    display: inline-block;
    transition:
      opacity 0.3s ease,
      transform 0.3s ease,
      filter 0.3s ease;
    opacity: 1;
    transform: translateY(0);
    filter: blur(0);
  }

  .phrase-rotator.transitioning {
    opacity: 0;
    transform: translateY(-8px);
    filter: blur(4px);
  }

  .inline-form {
    width: 400px;
    max-width: 90vw;
    animation: reveal-form 0.35s ease both;
  }

  @keyframes reveal-form {
    from {
      clip-path: inset(-8px -8px 100% -8px);
    }
    to {
      clip-path: inset(-8px);
    }
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

  .splash-close {
    padding-bottom: 32px;
    z-index: 1;
  }

  .close-circle {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    border: 1px solid var(--border-muted);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      color 0.15s ease,
      background-color 0.15s ease;
  }

  .close-circle:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }
</style>
