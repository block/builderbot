<!--
  SessionModal.svelte — Dialog chrome for the session chat pane.

  The chat transcript/input live in SessionChatPane so note dialogs can embed
  the same session UI beside note content.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import InContentSearch from '../../shared/InContentSearch.svelte';
  import { registerSearchShortcutTarget } from '../keyboard/searchTargets';
  import { viewport } from '../../shared/viewport.svelte';
  import ReferenceNavControls from '../references/ReferenceNavControls.svelte';
  import type { HashtagClickInfo, ReferenceNavState } from '../references/referenceHistory.svelte';
  import type { HashtagItem, ProjectRepo, Session } from '../../types';
  import type { LinkedNoteContext } from './noteFreshness';
  import type { DisplayRootInput } from './pathDisplayRoots';
  import SessionChatPane from './SessionChatPane.svelte';
  import { stripXmlTags } from './sessionModalHelpers';

  interface Props {
    open: boolean;
    sessionId: string;
    onClose: () => void;
    /** Display roots — tool call paths within them are shown as relative. */
    repoDir?: DisplayRootInput;
    /** Branch ID — when provided, enables image attachment on replies. */
    branchId?: string | null;
    /** Project ID — when provided, enables image attachment on replies. */
    projectId?: string | null;
    /** Repo label for grouping branch-scoped hashtag suggestions. */
    repoLabel?: Pick<ProjectRepo, 'githubRepo' | 'subpath' | 'headRepo'> | null;
    /** Precomputed hashtag reference items for the session context. */
    hashtagItems?: HashtagItem[];
    /** When set, shows a button to open the associated note. */
    noteInfo?: LinkedNoteContext | null;
    onOpenNote?: (note: LinkedNoteContext) => void;
    referenceNav?: ReferenceNavState;
    onHashtagClick?: (click: HashtagClickInfo) => void;
  }

  type SearchablePane = {
    performSearch: (query: string) => void;
    nextMatch: () => void;
    previousMatch: () => void;
    clearSearch: () => void;
    close: () => void;
  };

  let {
    open,
    sessionId,
    onClose,
    repoDir,
    branchId,
    projectId,
    repoLabel = null,
    hashtagItems,
    noteInfo,
    onOpenNote,
    referenceNav,
    onHashtagClick,
  }: Props = $props();

  let pane = $state<SearchablePane | null>(null);
  let session = $state<Session | null>(null);
  let searchVisible = $state(false);
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let unregisterSearchTarget: (() => void) | null = null;

  $effect(() => {
    if (!open) return;
    const unregister = registerSearchShortcutTarget({
      find: openSearch,
      next: nextMatch,
      previous: previousMatch,
    });
    unregisterSearchTarget = unregister;
    return () => {
      unregister();
      unregisterSearchTarget = null;
    };
  });

  onDestroy(() => {
    unregisterSearchTarget?.();
  });

  function openSearch() {
    searchVisible = true;
  }

  function closeSearch() {
    searchVisible = false;
    pane?.clearSearch();
    matchCount = 0;
    currentMatchIndex = 0;
  }

  function performSearch(query: string) {
    pane?.performSearch(query);
  }

  function nextMatch() {
    pane?.nextMatch();
  }

  function previousMatch() {
    pane?.previousMatch();
  }

  function requestClose() {
    pane?.close();
    onClose();
  }
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && requestClose()}>
  <Dialog.Content
    class="sm:max-w-[700px] h-[80vh] max-h-[900px] p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
    onOpenAutoFocus={(e) => e.preventDefault()}
  >
    <header class="modal-header">
      {#if referenceNav}
        <ReferenceNavControls nav={referenceNav} />
      {/if}
      <div class="header-content">
        <Dialog.Title
          class="text-[var(--size-sm)] font-semibold text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
        >
          {session?.prompt ? stripXmlTags(session.prompt) || 'Session' : 'Session'}
        </Dialog.Title>
      </div>
      <InContentSearch
        visible={searchVisible}
        {matchCount}
        currentIndex={currentMatchIndex}
        onSearch={performSearch}
        onNext={nextMatch}
        onPrevious={previousMatch}
        onClose={closeSearch}
      />
      <div class="header-actions">
        {#if noteInfo?.content.trim() && onOpenNote}
          <Button
            variant="outline"
            size="sm"
            class="h-7 shrink-0 px-2.5 text-xs text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[700px]:h-10"
            onclick={() => onOpenNote?.(noteInfo!)}
          >
            View note
          </Button>
        {/if}
        <Button
          variant="ghost"
          size="icon"
          class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[700px]:size-10 [&_svg]:!size-4"
          title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
          aria-label="Close"
          onclick={requestClose}
        >
          <X size={16} />
        </Button>
      </div>
    </header>

    <SessionChatPane
      bind:this={pane}
      active={open}
      {sessionId}
      {repoDir}
      {branchId}
      {projectId}
      {repoLabel}
      {hashtagItems}
      {noteInfo}
      {onHashtagClick}
      onSessionChange={(next) => (session = next)}
      onSearchStateChange={(state) => {
        matchCount = state.matchCount;
        currentMatchIndex = state.currentIndex;
      }}
    />
  </Dialog.Content>
</Dialog.Root>

<style>
  .modal-header {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  @media (max-width: 700px) {
    .modal-header {
      padding: 12px;
    }
  }
</style>
