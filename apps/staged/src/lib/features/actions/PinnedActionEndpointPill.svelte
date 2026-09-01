<!--
  PinnedActionEndpointPill.svelte — the shape a pinned run action takes once
  it's serving and has reported an endpoint: a status half that opens the
  output modal (or stops the action on alt-click) joined to a copy-URL half.

  Purely presentational: the parent decides when a pill is the right shape and
  what pressing it does, so this component never sees the runner or the
  execution. The only state it owns is the copied tick, which belongs to the
  one execution it renders.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { fade } from 'svelte/transition';
  import Check from '@lucide/svelte/icons/check';
  import Copy from '@lucide/svelte/icons/copy';
  import { Button } from '$lib/components/ui/button';
  import ActionStatusIcon from './ActionStatusIcon.svelte';
  import { actionStatusLabels } from './actionStatusLabels';

  interface Props {
    actionName: string;
    /** The URL the copy button puts on the clipboard. */
    copyUrl: string;
    /** A stop has been requested and the process hasn't exited yet. */
    stopping: boolean;
    /** The status half is currently offering to stop the action (alt held). */
    showStop: boolean;
    /** Outline surface theme (the repo card's badge-hued variant). */
    outline: boolean;
    /** Pressing the status half. */
    onPress: () => void;
  }

  let { actionName, copyUrl, stopping, showStop, outline, onPress }: Props = $props();

  // The pill only exists while its execution is running, so the labels take
  // running=true and never reach the completed/failed rungs.
  let labels = $derived(actionStatusLabels({ actionName, stopping, showStop, running: true }));

  let copied = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  function copyEndpoint(e: MouseEvent): void {
    e.stopPropagation();
    if (!copyUrl) return;
    navigator.clipboard.writeText(copyUrl).catch(() => {});
    if (copiedTimer) clearTimeout(copiedTimer);
    copied = true;
    copiedTimer = setTimeout(() => {
      copied = false;
      copiedTimer = undefined;
    }, 1500);
  }

  onDestroy(() => {
    if (copiedTimer) clearTimeout(copiedTimer);
  });
</script>

<div class="pinned-action-pill" class:outline>
  <Button
    variant="ghost"
    class={[
      'size-7 rounded-full border-0 bg-transparent text-muted-foreground hover:bg-transparent hover:text-foreground [&_svg]:!size-3.5',
      stopping && 'opacity-60',
      showStop && 'text-destructive',
    ]}
    title={labels.title}
    aria-label={labels.ariaLabel}
    onclick={onPress}
  >
    <ActionStatusIcon {stopping} {showStop} serving running size={14} />
  </Button>
  <Button
    variant="ghost"
    class={[
      'relative size-7 rounded-none border-0 border-l bg-transparent text-muted-foreground hover:text-foreground [&_svg]:!size-3',
      outline
        ? 'border-l-[var(--card-border-hover)] hover:bg-[var(--card-bg-strong)]'
        : 'border-l-[var(--border-muted)] hover:bg-[var(--bg-elevated)]',
    ]}
    title={`Copy endpoint: ${copyUrl}`}
    aria-label="Copy endpoint"
    onclick={copyEndpoint}
  >
    {#if copied}
      <span class="copy-icon-wrapper" in:fade={{ duration: 150 }} out:fade={{ duration: 150 }}>
        <Check size={12} />
      </span>
    {:else}
      <span class="copy-icon-wrapper" in:fade={{ duration: 150 }} out:fade={{ duration: 150 }}>
        <Copy size={12} />
      </span>
    {/if}
  </Button>
</div>

<style>
  .pinned-action-pill {
    display: flex;
    align-items: center;
    height: 28px;
    background: var(--bg-hover);
    border-radius: 999px;
    overflow: hidden;
  }

  .pinned-action-pill.outline {
    background: transparent;
    border: 1px solid var(--card-border-hover);
    box-sizing: border-box;
  }

  .copy-icon-wrapper {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
  }
</style>
