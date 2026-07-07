<script lang="ts">
  import { onMount } from 'svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import ImageViewerModal from '../timeline/ImageViewerModal.svelte';
  import { openDiffRoute } from '../layout/navigation.svelte';
  import type { HashtagItem } from '../../types';
  import {
    clearReferenceHistory,
    currentReferenceEntry,
    moveReferenceBack,
    moveReferenceForward,
    pushReferenceEntry,
    referenceCanGoBack,
    referenceCanGoForward,
    replaceCurrentReferenceEntry,
    resolveHashtagReference,
    restorePreviousReferenceAfterDiffRoute,
    setCurrentNoteView,
    type HashtagClickInfo,
    type ReferenceChatEntry,
    type ReferenceHistoryEntry,
    type ReferenceNoteEntry,
    type ReferenceNavState,
  } from './referenceHistory.svelte';

  let entry = $derived(currentReferenceEntry());
  let referenceNav = $derived<ReferenceNavState>({
    canGoBack: referenceCanGoBack(),
    canGoForward: referenceCanGoForward(),
    onBack: navigateBack,
    onForward: navigateForward,
  });

  onMount(() => {
    const handleDiffRoutePopped = () => {
      restorePreviousReferenceAfterDiffRoute();
    };
    window.addEventListener('staged:diff-route-popped', handleDiffRoutePopped);
    return () => {
      window.removeEventListener('staged:diff-route-popped', handleDiffRoutePopped);
    };
  });

  $effect(() => {
    if (entry?.kind !== 'chat') return;
    const noteEntry = noteEntryForChat(entry);
    if (!noteEntry) return;
    replaceCurrentReferenceEntry(noteEntry);
  });

  function navigateBack() {
    activateEntry(moveReferenceBack());
  }

  function navigateForward() {
    activateEntry(moveReferenceForward());
  }

  function activateEntry(next: ReferenceHistoryEntry | null) {
    if (next?.kind === 'diff') {
      openDiffRoute(next.route);
    }
  }

  function handleClose() {
    clearReferenceHistory();
  }

  function handleHashtagClick(click: HashtagClickInfo) {
    const current = currentReferenceEntry();
    if (!current || current.kind === 'diff') return;

    const target = resolveHashtagReference(click, {
      hashtagItems: current.hashtagItems,
      diffContext: current.diffContext,
    });
    if (!target) return;

    pushReferenceEntry(target);
    activateEntry(target);
  }

  function handleOpenInnerSession(sessionId: string) {
    const current = currentReferenceEntry();
    if (!current || current.kind === 'diff' || current.kind === 'image') return;

    pushReferenceEntry({
      kind: 'chat',
      ref: `#chat:${sessionId}`,
      sessionId,
      branchId: current.branchId,
      projectId: current.projectId,
      repoDir: current.repoDir,
      repoLabel: current.repoLabel,
      hashtagItems: current.hashtagItems,
      diffContext: current.diffContext,
    });
  }

  function noteEntryForChat(entry: ReferenceChatEntry): ReferenceNoteEntry | null {
    const note = noteItemForSession(entry.sessionId, entry.hashtagItems);
    if (!note?.noteContent && note?.noteContent !== '') return null;
    return {
      kind: 'note',
      noteKind: note.type === 'project-note' ? 'project' : 'branch',
      id: note.id,
      ref: note.type === 'project-note' ? `#project-note:${note.id}` : `#note:${note.id}`,
      title: note.title,
      content: note.noteContent,
      view: 'chat',
      sessionId: entry.sessionId,
      noteUpdatedAt: note.noteUpdatedAt,
      branchId: note.branchId ?? entry.branchId,
      projectId: note.projectId ?? entry.projectId,
      repoDir: entry.repoDir,
      repoLabel: entry.repoLabel,
      hashtagItems: entry.hashtagItems,
      diffContext: entry.diffContext,
    };
  }

  function noteItemForSession(sessionId: string, items: HashtagItem[] | undefined) {
    return items?.find((item) => {
      return (
        (item.type === 'note' || item.type === 'project-note') && item.noteSessionId === sessionId
      );
    });
  }
</script>

{#if entry?.kind === 'note'}
  <NoteModal
    open={true}
    title={entry.title}
    content={entry.content}
    sessionId={entry.sessionId}
    noteUpdatedAt={entry.noteUpdatedAt}
    noteId={entry.id}
    noteKind={entry.noteKind}
    branchId={entry.branchId}
    projectId={entry.projectId}
    repoDir={entry.repoDir}
    repoLabel={entry.repoLabel}
    chatOpen={entry.view === 'chat'}
    onChatOpenChange={(open) => setCurrentNoteView(open ? 'chat' : 'note')}
    hashtagItems={entry.hashtagItems}
    {referenceNav}
    onOpenSession={handleOpenInnerSession}
    onClose={handleClose}
    onHashtagClick={handleHashtagClick}
  />
{:else if entry?.kind === 'chat'}
  {@const noteEntry = noteEntryForChat(entry)}
  {#if noteEntry}
    <NoteModal
      open={true}
      title={noteEntry.title}
      content={noteEntry.content}
      sessionId={noteEntry.sessionId}
      noteUpdatedAt={noteEntry.noteUpdatedAt}
      noteId={noteEntry.id}
      noteKind={noteEntry.noteKind}
      branchId={noteEntry.branchId}
      projectId={noteEntry.projectId}
      repoDir={noteEntry.repoDir}
      repoLabel={noteEntry.repoLabel}
      chatOpen={true}
      onChatOpenChange={(open) =>
        replaceCurrentReferenceEntry({ ...noteEntry, view: open ? 'chat' : 'note' })}
      hashtagItems={noteEntry.hashtagItems}
      {referenceNav}
      onOpenSession={handleOpenInnerSession}
      onClose={handleClose}
      onHashtagClick={handleHashtagClick}
    />
  {:else}
    <SessionModal
      open={true}
      sessionId={entry.sessionId}
      repoDir={entry.repoDir}
      branchId={entry.branchId}
      projectId={entry.projectId}
      repoLabel={entry.repoLabel}
      hashtagItems={entry.hashtagItems}
      {referenceNav}
      onOpenSession={handleOpenInnerSession}
      onClose={handleClose}
      onHashtagClick={handleHashtagClick}
    />
  {/if}
{:else if entry?.kind === 'image'}
  <ImageViewerModal
    open={true}
    imageId={entry.imageId}
    filename={entry.filename}
    {referenceNav}
    onClose={handleClose}
  />
{/if}
