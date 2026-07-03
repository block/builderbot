<script lang="ts">
  import { onMount } from 'svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import ImageViewerModal from '../timeline/ImageViewerModal.svelte';
  import { openDiffRoute } from '../layout/navigation.svelte';
  import type { LinkedNoteContext } from '../sessions/noteFreshness';
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
    type ReferenceNavState,
    type ReferenceNoteEntry,
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

  function handleOpenSession() {
    const current = currentReferenceEntry();
    if (current?.kind === 'note' && current.sessionId) {
      setCurrentNoteView('chat');
    }
  }

  function handleOpenNote(note: LinkedNoteContext) {
    const current = currentReferenceEntry();
    if (current?.kind === 'note') {
      setCurrentNoteView('note');
      return;
    }
    if (current?.kind !== 'chat') return;

    replaceCurrentReferenceEntry({
      kind: 'note',
      noteKind: current.branchId ? 'branch' : 'project',
      id: note.id,
      ref: `#note:${note.id}`,
      title: note.title,
      content: note.content,
      view: 'note',
      sessionId: current.sessionId,
      noteUpdatedAt: note.updatedAt,
      branchId: current.branchId,
      projectId: current.projectId,
      hashtagItems: current.hashtagItems,
      diffContext: current.diffContext,
    });
  }

  function noteInfoFor(entry: ReferenceNoteEntry): LinkedNoteContext {
    return {
      id: entry.id,
      title: entry.title,
      content: entry.content,
      updatedAt: entry.noteUpdatedAt ?? 0,
      hasParsedNote: !!entry.content.trim(),
    };
  }

  function chatNoteInfo(entry: ReferenceChatEntry): LinkedNoteContext | null {
    const note = noteItemForSession(entry.sessionId, entry.hashtagItems);
    if (!note?.noteContent && note?.noteContent !== '') return null;
    return {
      id: note.id,
      title: note.title,
      content: note.noteContent,
      updatedAt: note.noteUpdatedAt ?? 0,
      hasParsedNote: !!note.noteContent.trim(),
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

{#if entry?.kind === 'note' && entry.view === 'note'}
  <NoteModal
    open={true}
    title={entry.title}
    content={entry.content}
    sessionId={entry.sessionId}
    noteUpdatedAt={entry.noteUpdatedAt}
    hashtagItems={entry.hashtagItems}
    {referenceNav}
    onClose={handleClose}
    onOpenSession={entry.sessionId ? handleOpenSession : undefined}
    onHashtagClick={handleHashtagClick}
  />
{:else if entry?.kind === 'note' && entry.view === 'chat' && entry.sessionId}
  <SessionModal
    open={true}
    sessionId={entry.sessionId}
    branchId={entry.branchId}
    projectId={entry.projectId}
    noteInfo={noteInfoFor(entry)}
    {referenceNav}
    onOpenNote={handleOpenNote}
    onClose={handleClose}
    onHashtagClick={handleHashtagClick}
  />
{:else if entry?.kind === 'chat'}
  <SessionModal
    open={true}
    sessionId={entry.sessionId}
    repoDir={entry.repoDir}
    branchId={entry.branchId}
    projectId={entry.projectId}
    repoLabel={entry.repoLabel}
    noteInfo={chatNoteInfo(entry)}
    {referenceNav}
    onOpenNote={handleOpenNote}
    onClose={handleClose}
    onHashtagClick={handleHashtagClick}
  />
{:else if entry?.kind === 'image'}
  <ImageViewerModal
    open={true}
    imageId={entry.imageId}
    filename={entry.filename}
    {referenceNav}
    onClose={handleClose}
  />
{/if}
