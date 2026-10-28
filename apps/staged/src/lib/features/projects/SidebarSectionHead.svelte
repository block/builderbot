<!--
  SidebarSectionHead.svelte - Uppercase section header for the projects sidebar.

  "PINNED REPOS 3" / "ALL PROJECTS 12": a small-caps label with a faint count,
  plus a trailing slot for the section's tool (the All repos link, the Filter
  menu). With `onclick` the label is a navigation target — `active` marks the
  section whose view is showing with an accent dot, the way the old All
  Projects row highlighted.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    label: string;
    count?: number | string;
    /** Makes the label a button (e.g. navigate to the section's view). */
    onclick?: () => void;
    active?: boolean;
    trailing?: Snippet;
  }

  let { label, count, onclick, active = false, trailing }: Props = $props();
</script>

<div class="section-head">
  {#if onclick}
    <button type="button" class="label" class:active {onclick}>
      {label}
      {#if count !== undefined}<span class="count">{count}</span>{/if}
    </button>
  {:else}
    <span class="label static">
      {label}
      {#if count !== undefined}<span class="count">{count}</span>{/if}
    </span>
  {/if}
  {#if trailing}<span class="tools">{@render trailing()}</span>{/if}
</div>

<style>
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    min-width: 0;
    padding: 12px 0 4px 4px;
  }

  .tools {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    border: none;
    background: transparent;
    padding: 2px 0;
    color: var(--text-muted);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    white-space: nowrap;
    cursor: pointer;
  }

  .label.static {
    cursor: default;
  }

  .label:not(.static):hover,
  .label.active {
    color: var(--text-primary);
  }

  .label.active::after {
    content: '';
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--ui-accent);
  }

  .count {
    color: var(--text-faint);
    letter-spacing: 0;
  }
</style>
