<!--
  TimelineContextMenu.svelte — Shared context menu for timeline rows.

  Rendered once per timeline (not per row) to avoid N window listeners.
  The parent opens it by calling `open(event)` and provides an
  `onNewSessionReferring` callback for the hashtag action.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import { Copy, MessageSquarePlus } from 'lucide-svelte';
  import type { ContextMenuEvent } from './TimelineRow.svelte';
  import ContextMenu from '../../shared/menu/ContextMenu.svelte';
  import type { MenuItem } from '../../shared/menu/types';

  interface Props {
    onNewSessionReferring?: (hashtagRef: string) => void;
  }

  let { onNewSessionReferring }: Props = $props();

  let contextMenu = $state<ReturnType<typeof ContextMenu> | undefined>();

  function buildItems(event: ContextMenuEvent): MenuItem[] {
    const items: MenuItem[] = [];

    if (event.commitSha) {
      items.push({
        type: 'action',
        label: 'Copy SHA',
        icon: Copy,
        onSelect: () => {
          navigator.clipboard.writeText(event.commitSha!).catch(() => {});
        },
      });
    }

    if (event.hashtagRef && onNewSessionReferring) {
      items.push({
        type: 'action',
        label: 'New session referring to this',
        icon: MessageSquarePlus,
        onSelect: () => {
          onNewSessionReferring?.(event.hashtagRef!);
        },
      });
    }

    return items;
  }

  /** Open the context menu. Called by the parent when a row emits onContextMenu. */
  export async function open(event: ContextMenuEvent) {
    await tick();
    contextMenu?.open({
      x: event.x,
      y: event.y,
      items: buildItems(event),
      ariaLabel: 'Timeline actions',
    });
  }

  /** Close the context menu. */
  export function close() {
    contextMenu?.close();
  }
</script>

<ContextMenu bind:this={contextMenu} ariaLabel="Timeline actions" minWidth={140} />
