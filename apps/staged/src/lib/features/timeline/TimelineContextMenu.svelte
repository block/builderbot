<script lang="ts">
  import type { Snippet } from 'svelte';
  import Copy from '@lucide/svelte/icons/copy';
  import MessageSquarePlus from '@lucide/svelte/icons/message-square-plus';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as ContextMenu from '$lib/components/ui/context-menu';

  export type TimelineContextMenuAction = {
    key: string;
    commitSha?: string;
    hashtagRef?: string;
    deleteDisabledReason?: string;
    onDelete?: (opts?: { altKey: boolean }) => void;
  };

  interface Props {
    actions?: TimelineContextMenuAction[];
    onNewSessionReferring?: (hashtagRef: string) => void;
    children?: Snippet;
  }

  let { actions = [], onNewSessionReferring, children }: Props = $props();

  let open = $state(false);
  let activeActionKey = $state<string | null>(null);

  function hasVisibleAction(action: TimelineContextMenuAction): boolean {
    return (
      !!action.commitSha ||
      (!!action.hashtagRef && !!onNewSessionReferring) ||
      !!action.onDelete ||
      !!action.deleteDisabledReason
    );
  }

  let actionByKey = $derived.by(() => {
    const map = new Map<string, TimelineContextMenuAction>();
    for (const action of actions) {
      if (hasVisibleAction(action)) {
        map.set(action.key, action);
      }
    }
    return map;
  });
  let hasActions = $derived(actionByKey.size > 0);
  let activeAction = $derived(activeActionKey ? (actionByKey.get(activeActionKey) ?? null) : null);

  function actionForEvent(event: Event): TimelineContextMenuAction | null {
    const currentTarget = event.currentTarget;
    const target = event.target;
    if (!(currentTarget instanceof Element) || !(target instanceof Element)) return null;

    const row = target.closest('[data-timeline-context-menu-key]');
    if (!(row instanceof HTMLElement) || !currentTarget.contains(row)) return null;

    const key = row.dataset.timelineContextMenuKey;
    if (!key) return null;
    return actionByKey.get(key) ?? null;
  }

  function handleContextMenu(event: MouseEvent) {
    const action = actionForEvent(event);
    if (!action) {
      activeActionKey = null;
      open = false;
      event.preventDefault();
      return;
    }

    activeActionKey = action.key;
  }

  function handlePointerDown(event: PointerEvent) {
    if (event.pointerType === 'mouse') return;

    const action = actionForEvent(event);
    if (!action) {
      activeActionKey = null;
      open = false;
      event.preventDefault();
      return;
    }

    activeActionKey = action.key;
  }

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen) {
      activeActionKey = null;
    }
  }

  function handleCopySha() {
    const sha = activeAction?.commitSha;
    if (sha) {
      navigator.clipboard.writeText(sha).catch(() => {});
    }
  }

  function handleNewSessionReferring() {
    const ref = activeAction?.hashtagRef;
    if (ref) {
      onNewSessionReferring?.(ref);
    }
  }

  function handleDelete() {
    activeAction?.onDelete?.({ altKey: false });
  }

  $effect(() => {
    if (activeActionKey && !actionByKey.has(activeActionKey)) {
      activeActionKey = null;
      open = false;
    }
  });
</script>

{#if hasActions}
  <ContextMenu.Root bind:open onOpenChange={handleOpenChange}>
    <ContextMenu.Trigger
      class="contents"
      oncontextmenu={handleContextMenu}
      onpointerdown={handlePointerDown}
    >
      {@render children?.()}
    </ContextMenu.Trigger>
    <ContextMenu.Content class="min-w-[180px]">
      {#if activeAction}
        {#if activeAction.commitSha}
          <ContextMenu.Item onSelect={handleCopySha}>
            <Copy size={14} /> Copy SHA
          </ContextMenu.Item>
        {/if}
        {#if activeAction.hashtagRef && onNewSessionReferring}
          <ContextMenu.Item onSelect={handleNewSessionReferring}>
            <MessageSquarePlus size={14} /> New session referring to this
          </ContextMenu.Item>
        {/if}
        {#if activeAction.onDelete || activeAction.deleteDisabledReason}
          <ContextMenu.Item
            variant="destructive"
            disabled={!!activeAction.deleteDisabledReason}
            title={activeAction.deleteDisabledReason ?? 'Delete'}
            onSelect={handleDelete}
          >
            <Trash2 size={14} /> Delete
          </ContextMenu.Item>
        {/if}
      {/if}
    </ContextMenu.Content>
  </ContextMenu.Root>
{:else}
  {@render children?.()}
{/if}
