<!--
  NoteModal.svelte — Markdown viewer for a branch note

  Displays rendered markdown content in a clean modal.
  Read-only view.

  Callers pass the note they snapshotted at click time, so while the modal is
  open it subscribes to `notes-changed` and refetches on any write that could
  touch this note — the session runner rewriting it, an edit in another window,
  a delete-and-rewrite. Nothing here is editable, so a refresh can't clobber
  user input.

  Showing a note marks it read, and the header's toggle marks it read or
  unread; see `NoteReadTracker` for when the viewer marks it on its own.

  A note with a chat can be replied to a passage at a time (`NoteReplyQuotes`):
  selecting text offers a Reply button (`NoteSelectionReply`; on a phone it is
  docked above the bottom bar, out of the way of the system's edit menu), and
  each reply lands in the chat's draft as a quote block (see
  sessions/replyDrafts.ts) — straight into the composer when the chat shows
  beside the note, otherwise typed in the note and written to the stored
  draft as it goes, the user staying on the note to select the next passage. While the chat is hidden
  and the draft holds anything, the note's bottom bar offers it: one button
  that opens the chat with the composer. The docked Reply button stands in
  for it while a selection is being offered a reply.

  Every passage the draft quotes stays marked in the note
  (`NoteQuoteHighlights`, from `replyDraftPresence.quotes`) for as long as
  its block is in the draft: through a re-render and a reopen, until the
  block is removed or the draft is sent. Pressing a mark goes back to its
  reply: with the chat beside the note the caret lands in the block's reply
  in the composer, otherwise the reply field reopens by the passage with
  what was written, to edit; closed on an empty reply, the field takes the
  block out of the draft and the mark goes.

  A note that has been revised shows what the revision changed
  (`NoteDiffState`): markers beside the note (`NoteChangeBar`), each level
  with its change and scrolling with it, and lighting it on hover, as one
  rectangle painted behind the change's text and ending at its marker.
  Pressing one opens the note's previous version on the left
  (`NotePreviousPane`), the two scrolling together, where the window has
  room for two panes; elsewhere it scrolls to the change and keeps it lit.
  There every change has its rectangle in both panes, running to the gutter
  between them, where a band joins the two. The previous version takes the
  place the chat would, so opening either closes the other. Escape closes the previous version before
  it closes the note, and until it has, Escape in the find field closes the
  search and nothing else.
-->
<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import Mail from '@lucide/svelte/icons/mail';
  import MailOpen from '@lucide/svelte/icons/mail-open';
  import MessageCircle from '@lucide/svelte/icons/message-circle';
  import FileText from '@lucide/svelte/icons/file-text';
  import PanelRightClose from '@lucide/svelte/icons/panel-right-close';
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open';
  import PencilLine from '@lucide/svelte/icons/pencil-line';
  import * as Dialog from '$lib/components/ui/dialog';
  import {
    createDialogWidth,
    dialogWidthStyle,
    NOTE_DIALOG_MIN_WIDTH,
    NOTE_DIALOG_WIDTH_KEY,
  } from '$lib/components/ui/dialog/dialogWidth.svelte';
  import { Button } from '$lib/components/ui/button';
  import {
    countAssistantMessagesAfter,
    getBranchNoteBySession,
    getMediaDimensions,
    getNote,
    getProjectNote,
    getProjectNoteBySession,
    handleExternalLinkClick,
    markSessionChatRead,
  } from '../../api/commands';
  import { listenToEvent } from '../../transport';
  import { notesChangeAffectsNote } from './noteRefresh';
  import { noteRenderInputsReady } from './noteRenderReadiness';
  import { NoteReadTracker } from './noteReadTracker.svelte';
  import { noteChatSessionStarter, type NoteChatSessionStart } from './noteChatNewSession';
  import { formatChatButtonLabel, type LinkedNoteContext } from '../sessions/noteFreshness';
  import PooledSessionChatPane from '../sessions/PooledSessionChatPane.svelte';
  import type { DisplayRootInput } from '../sessions/pathDisplayRoots';
  import InContentSearch from '../../shared/InContentSearch.svelte';
  import { clearHighlights, scrollToMatch } from '../../shared/textHighlight';
  import { registerSearchShortcutTarget } from '../keyboard/searchTargets';
  import { viewport } from '../../shared/viewport.svelte';
  import { scrollRangeEndIntoView } from './noteSelection';
  import { trackNoteMediaArea, type NoteMediaAreaTracker } from './noteMediaArea';
  import '../../shared/markdown/diagramStyles.css';
  import { extractMarkdownDiagramFences } from '../../shared/markdown/diagramFormats';
  import MediaViewerModal from '../../shared/markdown/MediaViewerModal.svelte';
  import {
    getMarkdownMediaFromEvent,
    getMarkdownMediaImageFromEvent,
  } from '../../shared/markdown/mediaViewer';
  import { mediaPinchOpen, type OpenMarkdownMedia } from '../../shared/markdown/mediaPinch';
  import {
    collectStagedMediaImageIds,
    resolveStagedMediaUrl,
  } from '../../shared/markdown/mediaUrl';
  import '../../shared/markdown/mediaStyles.css';
  import { missingMedia } from '../../shared/markdown/missingMedia';
  import { pageVideos } from '../../shared/markdown/pageVideos';
  import { followSectionLink } from '../../shared/markdown/sectionLinks';
  import DiagramViewerModal from '../../shared/markdown/DiagramViewerModal.svelte';
  import {
    getMarkdownDiagramSvgMarkup,
    isMarkdownDiagramActivationKey,
  } from '../../shared/markdown/diagramViewer';
  import { loadPikchrRenderer, type PikchrRenderer } from '../../shared/markdown/pikchrRendering';
  import { noteMarkdownWithTitle, renderNoteMarkdown } from './noteMarkdown';
  import type {
    HashtagItem,
    ImageDimensions,
    NotesChangedEvent,
    ProjectRepo,
    Session,
  } from '../../types';
  import { findHashtagItemForReference, renderHashtagTokens } from '../sessions/hashtagItems';
  import ReferenceNavControls from '../references/ReferenceNavControls.svelte';
  import NoteTransferMenu from './NoteTransferMenu.svelte';
  import NoteReplyQuotes from './NoteReplyQuotes.svelte';
  import NoteChangeBar from './NoteChangeBar.svelte';
  import NoteDiffPaneHeader from './NoteDiffPaneHeader.svelte';
  import NotePreviousPane from './NotePreviousPane.svelte';
  import { CONNECTOR_CURRENT_REACH } from './noteDiffConnectors';
  import { NoteDiffState } from './noteDiffState.svelte';
  import './noteDiffStyles.css';
  import { highlightNoteMatches } from './noteQuoteHighlights';
  import { replyDraftPresence } from '../sessions/replyDrafts';
  import { formatReplyDraftLabel } from './replyDraftLabel';
  import { isSessionActive } from '../../shared/sessionStatus';
  import type { HashtagClickInfo, ReferenceNavState } from '../references/referenceHistory.svelte';

  /**
   * Width the chat column takes when the split is open, added on top of the
   * persisted note width so opening chat preserves it when space permits. Matches the
   * second grid column in `.split-chat-open` below.
   */
  const CHAT_PANE_WIDTH = 380;

  interface Props {
    open: boolean;
    title: string;
    content: string;
    onClose: () => void;
    /** When set, shows a button to open the associated chat session. */
    sessionId?: string | null;
    /**
     * The session's status as the host knows it (its list row); the chat
     * pane's report wins while it is open. Queued/running disables "Move".
     */
    sessionStatus?: string | null;
    /** A project note's child notes, which a move brings along. */
    hasChildren?: boolean;
    noteUpdatedAt?: number | null;
    onOpenSession?: (sessionId: string) => void;
    noteId?: string | null;
    noteKind?: 'branch' | 'project';
    branchId?: string | null;
    projectId?: string | null;
    repoDir?: DisplayRootInput;
    repoLabel?: Pick<ProjectRepo, 'githubRepo' | 'subpath' | 'headRepo'> | null;
    chatOpen?: boolean;
    onChatOpenChange?: (open: boolean) => void;
    /** Suggested next steps to show as action buttons at the bottom. */
    nextSteps?: { commitStep: string | null; noteStep: string | null } | null;
    /** Called when the user clicks a next-step button. */
    onStartSession?: (mode: 'commit' | 'note', prefill: string) => void;
    /**
     * Start a new note or commit session about this note from the chat
     * composer's draft ("New note" / "New commit" in the Send button's
     * dropdown). The modal closes itself (`onClose`) first; the prompt
     * already refers to the note. Resolves whether the session started, so
     * the composer can put the draft back if it did not. Hosts that have no
     * branch to start on leave it unset, and it is never offered for project
     * notes: the dropdown then lacks the actions.
     */
    onStartSessionFromChat?: NoteChatSessionStart;
    /** The host cannot start a session right now: the actions show disabled. */
    startSessionFromChatDisabled?: boolean;
    hashtagItems?: HashtagItem[];
    referenceNav?: ReferenceNavState;
    onHashtagClick?: (click: HashtagClickInfo) => void;
  }

  let {
    open,
    title,
    content,
    onClose,
    sessionId,
    sessionStatus = null,
    hasChildren = false,
    noteUpdatedAt,
    onOpenSession,
    noteId,
    noteKind = 'branch',
    branchId,
    projectId,
    repoDir,
    repoLabel = null,
    chatOpen = $bindable(false),
    onChatOpenChange,
    nextSteps,
    onStartSession,
    onStartSessionFromChat,
    startSessionFromChatDisabled = false,
    hashtagItems = [],
    referenceNav,
    onHashtagClick,
  }: Props = $props();

  let copied = $state(false);
  let liveNote = $state<{ id: string; title: string; content: string; updatedAt: number } | null>(
    null
  );
  // Ordering guard for overlapping refreshes — a burst of writes fires a burst
  // of events, and only the newest fetch may write `liveNote`.
  let liveNoteRefreshSeq = 0;
  let assistantMessagesAfterNote = $state(0);
  let assistantMessagesAfterNoteLoadedKey = $state<string | null>(null);
  let chatAutoOpenDecisionKey = $state<string | null>(null);
  let chatButtonLabel = $derived(formatChatButtonLabel(assistantMessagesAfterNote));
  let canOpenSession = $derived(Boolean(sessionId));
  let replyDraftSummary = $derived(replyDraftPresence.summary(sessionId));
  let replyDraftLabel = $derived(formatReplyDraftLabel(replyDraftSummary));
  /** Bumped when find-in-note has rewritten text under the quotes' marks, so they are put back. */
  let quoteMarkRefresh = $state(0);
  let quoteMarkRefreshes = 0;
  /** Written without being read, so an effect that clears a search isn't re-run by it. */
  function refreshQuoteMarks() {
    quoteMarkRefresh = ++quoteMarkRefreshes;
  }
  let showChatInfo = $derived(canOpenSession && assistantMessagesAfterNote > 0 && !chatOpen);
  let displayTitle = $derived(liveNote?.title ?? title);
  let displayContent = $derived(liveNote?.content ?? content);
  let displayUpdatedAt = $derived(liveNote?.updatedAt ?? noteUpdatedAt);
  let displayNoteId = $derived(liveNote?.id ?? noteId);
  let hasNoteContent = $derived(!!displayContent.trim());
  let startSessionFromChat = $derived(
    noteChatSessionStarter({
      noteId: displayNoteId,
      noteKind,
      start: onStartSessionFromChat,
      dismiss: () => onClose(),
    })
  );
  let noteFreshnessKey = $derived(
    open && sessionId && typeof displayUpdatedAt === 'number'
      ? `${displayNoteId ?? 'note'}:${sessionId}:${displayUpdatedAt}`
      : null
  );
  let noteMarkdown = $derived(noteMarkdownWithTitle(displayTitle, displayContent));
  let splitChatOpen = $derived(chatOpen && viewport.canSplit && hasNoteContent);
  const noteDiff = new NoteDiffState();
  /** The text the note held before its latest revision, when it has been revised. */
  let previousVersion = $derived(open ? noteDiff.previous : null);
  /** The previous version shows beside the note: asked for, and there is room for both. */
  let splitDiffOpen = $derived(
    noteDiff.open && viewport.canSplit && !chatOpen && hasNoteContent && previousVersion !== null
  );
  /** How many note-width columns the dialog holds. */
  let noteColumns = $derived(splitDiffOpen ? 2 : 1);
  let chatOnly = $derived(chatOpen && (!viewport.canSplit || !hasNoteContent));
  /**
   * The docked Reply button (a phone, a selection standing) covers the
   * bottom bar. The bar stays in the layout under it, only hidden: taking
   * it out would give its height to the note's scroll box, whose offset is
   * then clamped by that much — the text moving under the finger that is
   * still extending the selection, which lands on whatever arrives there.
   */
  let replyDocked = $state(false);
  /** The chat is hidden and its draft holds something: the bottom bar offers it. */
  let showReplyDraft = $derived(canOpenSession && !chatOpen && replyDraftLabel !== null);
  let showBottomBar = $derived(
    showReplyDraft ||
      showChatInfo ||
      Boolean(nextSteps && onStartSession && (nextSteps.noteStep || nextSteps.commitStep))
  );
  const readTracker = new NoteReadTracker();
  let readToggleLabel = $derived(readTracker.unread ? 'Mark as read' : 'Mark as unread');

  // Only the note column's width is persisted — shared with WriteNoteModal,
  // since editing opens from here.
  const dialogWidth = createDialogWidth({
    key: NOTE_DIALOG_WIDTH_KEY,
    minWidth: NOTE_DIALOG_MIN_WIDTH,
  });
  void dialogWidth.ensureHydrated();
  let resizing = $state(false);

  function handleWidthChange(total: number, commit: boolean) {
    dialogWidth.set((total - (splitChatOpen ? CHAT_PANE_WIDTH : 0)) / noteColumns, commit);
  }

  let noteSearchAvailable = $derived(hasNoteContent && !chatOnly);
  let chatToggleLabel = $derived(
    chatOpen
      ? viewport.canSplit
        ? 'Hide chat'
        : 'View note'
      : viewport.canSplit
        ? 'Show chat'
        : 'View chat'
  );
  let chatToggleAriaLabel = $derived(
    chatOpen
      ? viewport.canSplit
        ? 'Hide chat pane'
        : 'View note pane'
      : viewport.canSplit
        ? 'Show chat pane'
        : 'View chat pane'
  );
  // The note pane sizes to its content up to most of the window, so a short
  // note gets a short dialog and a long one scrolls inside a tall one. With the
  // chat pane open the height is pinned instead: its message list is a scroll
  // region with no natural height, and a dialog that grew as replies streamed
  // in would shift the composer under the pointer. It is pinned beside the
  // previous version too, whose pane scrolls on its own.
  // Keep chat toggles animated; the resize handle suspends this transition
  // while measuring and applying pointer, keyboard, and reset commands.
  let contentClass = $derived(
    `dialog-resize-gutter ${chatOpen || splitDiffOpen ? 'h-[90vh]' : 'max-h-[90vh]'} p-0 gap-0 overflow-hidden flex flex-col${
      resizing ? ' transition-none' : ' transition-[width] duration-150'
    }`
  );
  // The previous version gets a note column of its own; the window caps the sum.
  let totalWidth = $derived(
    dialogWidth.width * noteColumns + (splitChatOpen ? CHAT_PANE_WIDTH : 0)
  );
  let totalMinWidth = $derived(
    dialogWidth.minWidth * noteColumns + (splitChatOpen ? CHAT_PANE_WIDTH : 0)
  );
  let noteInfo = $derived<LinkedNoteContext | null>(
    displayNoteId
      ? {
          id: displayNoteId,
          title: displayTitle,
          content: displayContent,
          updatedAt: displayUpdatedAt ?? 0,
          hasParsedNote: hasNoteContent,
        }
      : null
  );
  let previousSessionStatus = $state<Session['status'] | null>(null);
  // The pane's report while the chat is open, the host's status otherwise.
  let sessionActive = $derived(isSessionActive(previousSessionStatus ?? sessionStatus));
  let noteHasPikchr = $derived(
    extractMarkdownDiagramFences(noteMarkdown).some((diagram) => diagram.language === 'pikchr')
  );
  let pikchrRenderer = $state<PikchrRenderer | null>(null);
  let pikchrRendererLoadKey = $derived(noteMarkdown);
  let pikchrRendererLoadFailedKey = $state<string | null>(null);
  let pikchrRendererLoadFailed = $derived(pikchrRendererLoadFailedKey === pikchrRendererLoadKey);
  // Stored size and density of the images the note embeds, so they render at
  // logical size (see mediaRendering.ts). Keyed by image id, which names
  // immutable bytes, so a lookup outlives the note it was made for; a miss
  // (video, unreadable file, failed lookup) is kept as null so it is not
  // asked again and renders on the CSS caps alone.
  let mediaDimensions = $state<Record<string, ImageDimensions | null>>({});
  /** The ids a lookup is out for, so no id is asked for twice. */
  const mediaDimensionsAsked = new Set<string>();
  let noteMediaImageIds = $derived(collectStagedMediaImageIds(noteMarkdown));
  let mediaDimensionsKnown = $derived(noteMediaImageIds.every((id) => id in mediaDimensions));
  // The previous version's images are looked up here too. Its ids arrive
  // while the note's own lookup may still be out, and that lookup is left to
  // answer: every answer is kept, whatever the viewer shows by then, since
  // it is true of the id for good. Dropping it to ask again with the new
  // ids would hold the note's first render back for a second round trip.
  $effect(() => {
    if (!open) return;
    const wanted = new Set([...noteMediaImageIds, ...previousMediaImageIds]);
    const missing = [...wanted].filter(
      (id) => !(id in mediaDimensions) && !mediaDimensionsAsked.has(id)
    );
    if (!missing.length) return;
    for (const id of missing) mediaDimensionsAsked.add(id);
    const record = (found: Record<string, ImageDimensions>) => {
      const next = { ...mediaDimensions };
      for (const id of missing) {
        next[id] = found[id] ?? null;
        mediaDimensionsAsked.delete(id);
      }
      mediaDimensions = next;
    };
    getMediaDimensions(missing).then(record, (error) => {
      console.warn('Failed to load note image dimensions', error);
      record({});
    });
  });
  // The note is not rendered until everything its render needs is in — its
  // images' dimensions (a local lookup) and, for a note with a pikchr
  // diagram, the lazily loaded renderer. `{@html}` rebuilds the DOM when the
  // string changes, so rendering first without an input and again once it
  // lands would reload every image, jump the ones already painted and remount
  // a video the reader had started (see noteRenderReadiness.ts). A note that
  // needs neither, or whose inputs are already cached, renders at once.
  let noteRenderReady = $derived(
    noteRenderInputsReady({
      mediaDimensionsKnown,
      needsPikchr: noteHasPikchr,
      pikchrRendererLoaded: pikchrRenderer !== null,
      pikchrRendererLoadFailed,
    })
  );
  function renderNote(markdown: string): string {
    return renderNoteMarkdown(markdown, {
      pikchrRenderer,
      renderInlineText: hashtagItems.length
        ? (text) => renderHashtagTokens(text, hashtagItems)
        : undefined,
      mediaDimensions: (id) => mediaDimensions[id],
    });
  }
  let renderedNoteHtml = $derived(noteRenderReady ? renderNote(noteMarkdown) : null);
  // The previous version is what the change bar compares the note with and
  // the side-by-side view shows. It is rendered as the note is, so it waits
  // on the same kinds of input, its own images' dimensions and the pikchr
  // renderer. The note never waits on these: its render is not held back
  // for a version it may not have.
  let previousMarkdown = $derived(
    previousVersion ? noteMarkdownWithTitle(previousVersion.title, previousVersion.content) : null
  );
  let previousHasPikchr = $derived(
    previousMarkdown !== null &&
      extractMarkdownDiagramFences(previousMarkdown).some(
        (diagram) => diagram.language === 'pikchr'
      )
  );
  let previousMediaImageIds = $derived(
    previousMarkdown === null ? [] : collectStagedMediaImageIds(previousMarkdown)
  );
  let renderedPreviousHtml = $derived(
    previousMarkdown !== null &&
      noteRenderInputsReady({
        mediaDimensionsKnown: previousMediaImageIds.every((id) => id in mediaDimensions),
        needsPikchr: previousHasPikchr,
        pikchrRendererLoaded: pikchrRenderer !== null,
        pikchrRendererLoadFailed,
      })
      ? renderNote(previousMarkdown)
      : null
  );
  let previousMarkdownEl = $state<HTMLDivElement>();
  /**
   * Media the note embeds is in the store. Media only the previous version
   * names may not be: it is deleted once nothing but a version names it, and
   * the version then shows a placeholder where it was (missingMedia.ts).
   */
  const noteEmbedsMedia = (id: string) => noteMarkdown.includes(id);
  let mediaViewer = $state<OpenMarkdownMedia | null>(null);
  let diagramViewerSvg = $state<string | null>(null);

  // Search state
  let searchVisible = $state(false);
  let searchQuery = $state('');
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let matchElements: HTMLElement[] = [];
  let contentEl = $state<HTMLDivElement>();
  let markdownEl = $state<HTMLDivElement>();
  /** The dialog's box, which the phone's reply sheet is laid over. */
  let dialogEl = $state<HTMLElement | null>(null);
  /** Where a phone's Reply button docks: over the note pane's bottom slot (see `.note-bottom`). */
  let dockEl = $state<HTMLElement | null>(null);
  /** The pane's bottom slot: the bottom bar, when there is one, with the dock laid over it. */
  let bottomSlotEl = $state<HTMLElement | null>(null);
  /**
   * The docked button's height while it shows, else 0, and the bottom
   * slot's. The button stands on the slot's bottom edge, so it covers the
   * slot first and only what is taller than the slot reaches up over the
   * note; the note's content is padded by that overlap
   * (`--selection-dock-overlap`) so the end of a selection near the bottom
   * of the note can scroll clear of the button rather than sit behind it.
   * Observed rather than measured on `replyDocked`, so they follow the
   * button in and out whatever the order of the renders.
   */
  let dockHeight = $state(0);
  let bottomSlotHeight = $state(0);
  let dockOverlap = $derived(Math.max(0, dockHeight - bottomSlotHeight));
  $effect(() => observeHeight(dockEl, (height) => (dockHeight = height)));
  $effect(() => observeHeight(bottomSlotEl, (height) => (bottomSlotHeight = height)));
  function observeHeight(el: HTMLElement | null, set: (height: number) => void) {
    if (!el || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => set(el.offsetHeight));
    observer.observe(el);
    return () => observer.disconnect();
  }
  // The button appears over the end of a selection made in the last lines of
  // the note (the padding above runs the content under it rather than
  // shortening it). Once the content is padded, the least scroll that puts
  // the end clear of the button. Keyed on the measured overlap rather than
  // `replyDocked`: that is the render the padding lands in. With a bottom
  // bar under the note the button covers the bar instead and nothing moves.
  let dockWasShown = false;
  $effect(() => {
    const overlap = dockOverlap;
    const appeared = overlap > 0 && !dockWasShown;
    dockWasShown = overlap > 0;
    if (!appeared) return;
    untrack(() => {
      const container = contentEl;
      const current = document.getSelection();
      if (!container || !markdownEl || !current || current.rangeCount === 0) return;
      const range = current.getRangeAt(0);
      if (current.isCollapsed || !markdownEl.contains(range.commonAncestorContainer)) return;
      scrollRangeEndIntoView(container, range, overlap);
    });
  });
  // The note's media is capped at a share of its scroll area's height
  // (mediaStyles.css), which this dialog's content-sized layout can't offer
  // as a size container; see noteMediaArea.ts. The tracker skips measuring
  // while the selection dock pads the area, so it is told when the dock's
  // overlap is back to 0 to take any measurement it owes. That is a separate
  // effect: re-creating the tracker on `dockOverlap` would clear the height
  // and leave the media on the fallback cap while the dock shows.
  let mediaArea: NoteMediaAreaTracker | undefined;
  $effect(() => {
    const tracker = trackNoteMediaArea(contentEl, dialogEl);
    mediaArea = tracker;
    return () => {
      tracker?.stop();
      if (mediaArea === tracker) mediaArea = undefined;
    };
  });
  $effect(() => {
    if (dockOverlap === 0) mediaArea?.refresh();
  });
  let unregisterSearchTarget: (() => void) | null = null;

  // Register the global search-shortcut target only while the visible note pane can be searched.
  // Branch and project views lazy-mount this component, but this guard keeps
  // persistent callers from letting a closed note modal capture Cmd/Ctrl+F.
  $effect(() => {
    if (!open || !noteSearchAvailable) return;
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

  $effect(() => {
    if (noteSearchAvailable || !searchVisible) return;
    closeSearch();
  });

  $effect(() => {
    const needed = noteHasPikchr || previousHasPikchr;
    if (!open || !needed || pikchrRenderer || pikchrRendererLoadFailed) return;

    const loadKey = pikchrRendererLoadKey;
    let stale = false;
    loadPikchrRenderer()
      .then((renderer) => {
        if (!stale && pikchrRendererLoadKey === loadKey) pikchrRenderer = renderer;
      })
      .catch(() => {
        if (!stale && pikchrRendererLoadKey === loadKey) pikchrRendererLoadFailedKey = loadKey;
      });

    return () => {
      stale = true;
    };
  });

  $effect(() => {
    if (!open) {
      pikchrRendererLoadFailedKey = null;
      chatAutoOpenDecisionKey = null;
      previousSessionStatus = null;
      readTracker.reset();
    }
  });

  // Showing the note marks it read: on open, and when reference navigation
  // swaps in another note. Only those — not refreshes or pane toggles — so a
  // manual "Mark as unread" sticks while the note stays open. A chat-only view
  // hasn't shown the note yet, so it waits for the note pane.
  $effect(() => {
    const id = open && !chatOnly ? displayNoteId : null;
    if (!id) return;
    const kind = noteKind;
    untrack(() => readTracker.show(kind, id));
  });

  // Fresh props supersede the override — and any refresh still in flight for
  // the note they replaced, since reference navigation swaps notes in place.
  $effect(() => {
    noteId;
    title;
    content;
    noteUpdatedAt;
    liveNote = null;
    liveNoteRefreshSeq++;
  });

  // The props are a snapshot taken when the note was clicked; the surfaces
  // behind the modal reload themselves off this same feed, but nothing maps
  // their fresh data back into those props. So refetch here on any write that
  // could touch this note — including one made while the chat pane is closed,
  // which is the only other refresh path.
  $effect(() => {
    if (!open) return;
    const target = { noteKind, branchId, projectId };
    return listenToEvent<NotesChangedEvent>('notes-changed', (payload) => {
      if (!notesChangeAffectsNote(payload, target)) return;
      void refreshLiveNote();
    });
  });

  onDestroy(() => {
    unregisterSearchTarget?.();
  });

  $effect(() => {
    const key = noteFreshnessKey;
    const sid = sessionId;
    const updatedAt = displayUpdatedAt;
    if (!open || !key || !sid || typeof updatedAt !== 'number') {
      assistantMessagesAfterNote = 0;
      assistantMessagesAfterNoteLoadedKey = null;
      return;
    }

    assistantMessagesAfterNoteLoadedKey = null;
    let stale = false;
    countAssistantMessagesAfter(sid, updatedAt)
      .then((count) => {
        if (!stale) {
          assistantMessagesAfterNote = count;
          assistantMessagesAfterNoteLoadedKey = key;
        }
      })
      .catch(() => {
        if (!stale) {
          assistantMessagesAfterNote = 0;
          assistantMessagesAfterNoteLoadedKey = key;
        }
      });

    return () => {
      stale = true;
    };
  });

  $effect(() => {
    const key = noteFreshnessKey;
    if (!key || assistantMessagesAfterNoteLoadedKey !== key || chatAutoOpenDecisionKey === key) {
      return;
    }

    chatAutoOpenDecisionKey = key;
    // Not over the previous version the reader has just opened in its place.
    if (assistantMessagesAfterNote > 0 && !chatOpen && !untrack(() => noteDiff.engaged)) {
      setChatOpen(true);
    }
  });

  // The note's previous version is fetched when the note opens and each time
  // it is fetched again: a revision replaces it. `updatedAt` moves on every
  // write of the note, so it asks again after those that left the text alone.
  $effect(() => {
    const id = open && hasNoteContent ? displayNoteId : null;
    const subject = id ? { id, title: displayTitle, content: displayContent } : null;
    void displayUpdatedAt;
    untrack(() => noteDiff.load(subject));
  });

  // The chat and the previous version take the same place beside the note,
  // one at a time: the chat opening, from here or from the host, closes the
  // previous version (see `handleDiffSelect` for the other way round).
  $effect(() => {
    if (chatOpen) untrack(() => noteDiff.close());
  });

  function handleDiffSelect(hunkId: string) {
    const split = viewport.canSplit;
    if (split && chatOpen) setChatOpen(false);
    noteDiff.select(hunkId, split);
  }

  /**
   * Escape lets go of the diff first: the previous version closes and the
   * note stays.
   *
   * The find field keeps its Escape to itself for as long as that holds
   * (`containEscape` below), so that closing the search does not also let go
   * of the change being read. With nothing of the diff held, one Escape in
   * the field closes the search and the note, as it does in the session
   * viewer and did before notes had a diff.
   */
  function handleEscapeKeydown(event: KeyboardEvent) {
    if (!noteDiff.engaged) return;
    event.preventDefault();
    noteDiff.close();
  }

  // Showing the chat marks its replies read, clearing the unread dot on the
  // note's row; hiding it (or closing the dialog) does too, covering replies
  // that streamed in while it was open.
  $effect(() => {
    const sid = open && chatOpen ? sessionId : null;
    if (!sid) return;
    markChatRead(sid);
    return () => markChatRead(sid);
  });

  function markChatRead(sid: string) {
    markSessionChatRead(sid).catch((error) => {
      console.warn('Failed to mark chat read', error);
    });
  }

  function setChatOpen(next: boolean) {
    chatOpen = next;
    onChatOpenChange?.(next);
    if (!next) {
      previousSessionStatus = null;
    }
  }

  /**
   * Fetch the current row for the note on screen.
   *
   * Session lookup comes first: it resolves a note this modal opened before the
   * runner had written it, which has no id yet. Otherwise fall back to the id,
   * which is all a reference-history entry for a session-less note carries.
   */
  async function fetchLatestNote() {
    if (sessionId) {
      return noteKind === 'project'
        ? await getProjectNoteBySession(sessionId)
        : await getBranchNoteBySession(sessionId);
    }
    const id = displayNoteId;
    if (!id) return null;
    return noteKind === 'project' ? await getProjectNote(id) : await getNote(id);
  }

  async function refreshLiveNote() {
    const seq = ++liveNoteRefreshSeq;
    try {
      const refreshed = await fetchLatestNote();
      // A later refresh already landed, or the note is gone — either way this
      // result is not the one to render.
      if (!refreshed || seq !== liveNoteRefreshSeq) return;
      liveNote = {
        id: refreshed.id,
        title: refreshed.title,
        content: refreshed.content,
        updatedAt: refreshed.updatedAt,
      };
      readTracker.observe(refreshed);
    } catch {
      // The next notes-changed event or reopen will try again.
    }
  }

  function handlePaneSessionChange(next: Session | null) {
    const wasActive = isSessionActive(previousSessionStatus);
    const isInactive = next && !isSessionActive(next.status);
    if ((wasActive && isInactive) || (!previousSessionStatus && isInactive && !hasNoteContent)) {
      void refreshLiveNote();
    }
    previousSessionStatus = next?.status ?? null;
  }

  /** The title heading the note is rendered under heads every path, so it is left out. */
  function isTitleHeading(heading: Element): boolean {
    return (
      !!displayTitle.trim() &&
      heading.tagName === 'H1' &&
      heading.parentElement === markdownEl &&
      heading === markdownEl?.firstElementChild
    );
  }

  function handleChatToggle() {
    if (!canOpenSession) return;
    if (chatOpen && !viewport.canSplit) {
      setChatOpen(false);
      return;
    }
    setChatOpen(!chatOpen);
  }

  async function handleShare() {
    try {
      await navigator.clipboard.writeText(noteMarkdown);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // clipboard API may fail in some contexts
    }
  }

  function openSearch() {
    if (!noteSearchAvailable) return;
    searchVisible = true;
  }

  function closeSearch() {
    searchVisible = false;
    searchQuery = '';
    if (contentEl) {
      clearHighlights(contentEl);
    }
    matchCount = 0;
    currentMatchIndex = 0;
    matchElements = [];
    refreshQuoteMarks();
  }

  function performSearch(query: string) {
    if (!contentEl) return;

    // Clear the previous highlights and highlight the new matches (none for
    // an empty query), over the note without its quotes' marks, so a match
    // can run across the edge of one. The marks are put back afterwards,
    // around and inside the matches. The note element is read untracked:
    // this runs in the search box's effect, which a new render of the note
    // must not re-run.
    const noteEl = untrack(() => markdownEl);
    const result = highlightNoteMatches(contentEl, noteEl, query);
    refreshQuoteMarks();
    matchElements = result.elements;
    matchCount = result.total;
    currentMatchIndex = 0;

    // Scroll to first match if any
    if (matchElements.length > 0) {
      scrollToMatch(matchElements[0]);
    }
  }

  function nextMatch() {
    if (matchElements.length === 0) return;

    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    currentMatchIndex = (currentMatchIndex + 1) % matchElements.length;

    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex]);
  }

  function previousMatch() {
    if (matchElements.length === 0) return;

    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    currentMatchIndex = (currentMatchIndex - 1 + matchElements.length) % matchElements.length;

    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex]);
  }

  function openMediaViewerFromEvent(event: MouseEvent | KeyboardEvent): boolean {
    const media = getMarkdownMediaFromEvent(event.target);
    if (!media) return false;
    event.preventDefault();
    event.stopPropagation();
    mediaViewer = { ...media, originImage: getMarkdownMediaImageFromEvent(event.target) };
    return true;
  }

  // Pinch-out on an inline image opens it too, following the fingers.
  const openMediaOnPinch = mediaPinchOpen((media) => (mediaViewer = media));

  function openDiagramViewerFromEvent(event: MouseEvent | KeyboardEvent): boolean {
    const svgMarkup = getMarkdownDiagramSvgMarkup(event.target);
    if (!svgMarkup) return false;

    event.preventDefault();
    event.stopPropagation();
    diagramViewerSvg = svgMarkup;
    return true;
  }

  function hashtagClickFromTarget(target: EventTarget | null): HashtagClickInfo | null {
    if (!(target instanceof HTMLElement)) return null;
    const badge = target.closest<HTMLElement>('[data-hashtag-ref]');
    if (!badge) return null;
    const type = badge.dataset.hashtagType as HashtagItem['type'] | undefined;
    const id = badge.dataset.hashtagId;
    const ref = badge.dataset.hashtagRef;
    if (!type || !id || !ref) return null;
    const item = findHashtagItemForReference(hashtagItems, type, id);
    return { type, id, ref, item };
  }

  function handleContentClick(event: MouseEvent) {
    if (openMediaViewerFromEvent(event) || openDiagramViewerFromEvent(event)) return;

    const click = hashtagClickFromTarget(event.target);
    if (click && onHashtagClick) {
      event.preventDefault();
      event.stopPropagation();
      onHashtagClick(click);
      return;
    }
    // A link to a heading of this note scrolls to it, looked up in this
    // note's own markdown: another note open behind may share the heading.
    if (followSectionLink(event, { content: markdownEl, scroller: contentEl })) return;
    handleExternalLinkClick(event);
  }

  function handleContentKeydown(event: KeyboardEvent) {
    if (
      isMarkdownDiagramActivationKey(event) &&
      (openMediaViewerFromEvent(event) || openDiagramViewerFromEvent(event))
    )
      return;
    if (event.key !== 'Enter' && event.key !== ' ') return;
    const click = hashtagClickFromTarget(event.target);
    if (!click || !onHashtagClick) return;
    event.preventDefault();
    event.stopPropagation();
    onHashtagClick(click);
  }
</script>

{#snippet copyButton()}
  <Button
    variant="outline"
    size="sm"
    class={[
      'touch-hit h-7 shrink-0 gap-1 border-[var(--border-muted)] bg-transparent px-2.5 text-xs text-muted-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground',
      copied && 'text-[var(--status-added)] hover:text-[var(--status-added)]',
    ]}
    title={copied ? 'Copied!' : 'Copy note to clipboard'}
    aria-label={copied ? 'Copied!' : 'Copy note to clipboard'}
    onclick={handleShare}
  >
    {#if copied}
      <Check size={16} />
    {:else}
      <Copy size={16} />
    {/if}
  </Button>
{/snippet}

{#snippet readToggleButton()}
  {#if readTracker.canToggle}
    <Button
      variant="outline"
      size="sm"
      class="touch-hit h-7 shrink-0 gap-1 border-[var(--border-muted)] bg-transparent px-2.5 text-xs text-muted-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
      title={readToggleLabel}
      aria-label={readToggleLabel}
      disabled={readTracker.toggling}
      onclick={() => readTracker.toggle()}
    >
      {#if readTracker.unread}
        <MailOpen size={16} />
      {:else}
        <Mail size={16} />
      {/if}
    </Button>
  {/if}
{/snippet}

{#snippet chatToggleButton()}
  {#if canOpenSession}
    <Button
      variant="outline"
      size="sm"
      class="touch-hit h-7 shrink-0 gap-1 border-[var(--border-muted)] bg-transparent px-2.5 text-xs text-muted-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
      title={chatToggleAriaLabel}
      aria-label={chatToggleAriaLabel}
      aria-pressed={chatOpen}
      onclick={handleChatToggle}
    >
      {#if chatOpen}
        <PanelRightClose size={15} aria-hidden="true" />
      {:else}
        <PanelRightOpen size={15} aria-hidden="true" />
      {/if}
      <span>{chatToggleLabel}</span>
    </Button>
  {/if}
{/snippet}

{#snippet closeButton()}
  <Button
    variant="ghost"
    size="icon-sm"
    class="touch-hit size-7 shrink-0 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
    title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
    aria-label="Close"
    onclick={onClose}
  >
    <X size={16} />
  </Button>
{/snippet}

{#snippet previousContent()}
  {#if renderedPreviousHtml !== null}
    <div
      class="markdown-content"
      bind:this={previousMarkdownEl}
      {@attach pageVideos(renderedPreviousHtml)}
      {@attach missingMedia(renderedPreviousHtml, noteEmbedsMedia)}
    >
      {@html renderedPreviousHtml}
    </div>
  {/if}
{/snippet}

<Dialog.Root
  {open}
  onOpenChange={(v) => {
    if (!v) {
      if (searchVisible) {
        closeSearch();
      }
      onClose();
    }
  }}
>
  <Dialog.Content
    bind:ref={dialogEl}
    class={contentClass}
    style={dialogWidthStyle(totalWidth)}
    showCloseButton={false}
    onOpenAutoFocus={(e) => e.preventDefault()}
    onEscapeKeydown={handleEscapeKeydown}
  >
    <Dialog.Header
      class="gap-0 border-b border-[var(--border-subtle)] p-0 flex-shrink-0 {chatOnly
        ? 'note-modal-chat-header'
        : ''}"
    >
      <div class:split-chat-open={splitChatOpen} class="note-modal-header-grid">
        <div class="note-header-pane" class:split-pane={splitChatOpen}>
          {#if referenceNav}
            <ReferenceNavControls nav={referenceNav} />
          {/if}
          <div class="header-content">
            <span class="note-title-icon" aria-hidden="true">
              <FileText size={13} />
            </span>
            <Dialog.Title
              class="text-[var(--size-sm)] font-semibold text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
            >
              Note
            </Dialog.Title>
          </div>
          {#if noteSearchAvailable}
            <InContentSearch
              visible={searchVisible}
              {matchCount}
              currentIndex={currentMatchIndex}
              containEscape={noteDiff.engaged}
              onSearch={performSearch}
              onNext={nextMatch}
              onPrevious={previousMatch}
              onClose={closeSearch}
            />
          {/if}
          <div class="note-header-actions touch-hit-group">
            {#if displayNoteId}
              <NoteTransferMenu
                noteId={displayNoteId}
                {noteKind}
                noteTitle={displayTitle}
                {projectId}
                {hasChildren}
                {sessionActive}
                onMoved={onClose}
              />
            {/if}
            {@render readToggleButton()}
            {#if hasNoteContent}
              {@render copyButton()}
            {/if}
            {#if !splitChatOpen && (hasNoteContent || !chatOpen)}
              {@render chatToggleButton()}
            {/if}
          </div>
        </div>
        {#if splitChatOpen}
          <div class="chat-header-pane touch-hit-group">
            {@render chatToggleButton()}
            {@render closeButton()}
          </div>
        {:else}
          <div class="header-actions">
            {@render closeButton()}
          </div>
        {/if}
      </div>
    </Dialog.Header>
    <div
      class:split-chat-open={splitChatOpen}
      class:split-diff-open={splitDiffOpen}
      class:chat-only={chatOnly}
      class="modal-body"
    >
      {#if splitDiffOpen && previousVersion}
        <NotePreviousPane
          {noteDiff}
          contentEl={previousMarkdownEl}
          renderKey={renderedPreviousHtml}
          savedAt={previousVersion.savedAt}
          currentScrollEl={contentEl}
          bottomInset={bottomSlotHeight}
          onClose={() => noteDiff.close()}
          onSelect={handleDiffSelect}
          onContentClick={handleContentClick}
          onContentKeydown={handleContentKeydown}
          content={previousContent}
        />
      {/if}
      <section
        id="note-modal-note-pane"
        class="note-pane"
        aria-hidden={chatOnly}
        aria-label="Note content"
      >
        {#if splitDiffOpen}
          <NoteDiffPaneHeader label="Current version" />
        {/if}
        <div class="note-scroll" style:--selection-dock-overlap="{dockOverlap}px">
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="modal-content"
            class:ends-pane={!showBottomBar}
            class:note-diff-all={splitDiffOpen}
            style:--note-diff-blocks-reach="{CONNECTOR_CURRENT_REACH}px"
            bind:this={contentEl}
            onclick={handleContentClick}
            {@attach openMediaOnPinch}
            onkeydown={handleContentKeydown}
          >
            {#if hasNoteContent}
              {#if renderedNoteHtml !== null}
                <div
                  class="markdown-content"
                  bind:this={markdownEl}
                  {@attach pageVideos(renderedNoteHtml)}
                >
                  {@html renderedNoteHtml}
                </div>
              {/if}
            {:else}
              <p class="empty-note">This note has no content.</p>
            {/if}
            {#if sessionId && hasNoteContent}
              <NoteReplyQuotes
                {sessionId}
                scrollEl={contentEl}
                contentEl={markdownEl}
                renderKey={renderedNoteHtml}
                refreshKey={quoteMarkRefresh}
                enabled={open && !chatOnly && renderedNoteHtml !== null}
                {chatOpen}
                chatVisible={splitChatOpen}
                sheetHost={dialogEl}
                dockHost={dockEl}
                onDockedChange={(docked) => (replyDocked = docked)}
                ignoreHeading={isTitleHeading}
              />
            {/if}
            {#if hasNoteContent}
              <NoteChangeBar
                {noteDiff}
                contentEl={markdownEl}
                scrollEl={contentEl}
                renderKey={renderedNoteHtml}
                previousHtml={renderedPreviousHtml}
                showAll={splitDiffOpen}
                onSelect={handleDiffSelect}
              />
            {/if}
          </div>
        </div>

        <!-- The pane's bottom slot: the bottom bar, and over it the place a
             phone's Reply button docks (NoteSelectionReply). The bar keeps its
             height under the button so the note's scroll box doesn't change. -->
        <div class="note-bottom" bind:this={bottomSlotEl}>
          {#if showBottomBar}
            <div class="next-steps" class:is-covered={replyDocked} aria-hidden={replyDocked}>
              {#if showReplyDraft || showChatInfo}
                <div class="chat-info-row">
                  {#if showReplyDraft}
                    <Button
                      variant="outline"
                      class="reply-draft-button h-auto min-h-9 max-w-full gap-2 border-[var(--border-muted)] bg-[var(--bg-chrome)] px-3.5 py-2 text-sm font-medium text-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground mobile:min-h-[44px]"
                      title={viewport.canSplit
                        ? 'Open the chat to finish your reply'
                        : 'View the chat to finish your reply'}
                      onclick={() => setChatOpen(true)}
                    >
                      <PencilLine size={16} class="text-[var(--ui-accent)]" aria-hidden="true" />
                      <span class="min-w-0 truncate">{replyDraftLabel}</span>
                    </Button>
                  {/if}
                  {#if showChatInfo}
                    <Button
                      variant="outline"
                      class="h-auto min-h-9 max-w-full gap-2 border-[var(--border-muted)] bg-[var(--bg-chrome)] px-3.5 py-2 text-sm font-medium text-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground mobile:min-h-[44px]"
                      onclick={() => setChatOpen(true)}
                    >
                      <MessageCircle size={16} aria-hidden="true" />
                      <span class="min-w-0 truncate" title={chatButtonLabel}>{chatButtonLabel}</span
                      >
                    </Button>
                  {/if}
                </div>
              {/if}
              {#if nextSteps && onStartSession && nextSteps.noteStep}
                <div class="next-step-row">
                  <span class="next-step-prompt">{nextSteps.noteStep}</span>
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-auto shrink-0 rounded-md border-transparent bg-[var(--note-bg)] px-3 py-1 text-xs font-medium text-[var(--note-color)] shadow-none hover:border-transparent hover:bg-[var(--note-bg-emphasis)] hover:text-[var(--note-color)]"
                    onclick={() => onStartSession('note', nextSteps!.noteStep!)}
                  >
                    Start note
                  </Button>
                </div>
              {/if}
              {#if nextSteps && onStartSession && nextSteps.commitStep}
                <div class="next-step-row">
                  <span class="next-step-prompt">{nextSteps.commitStep}</span>
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-auto shrink-0 rounded-md border-transparent bg-[var(--commit-bg)] px-3 py-1 text-xs font-medium text-[var(--commit-color)] shadow-none hover:border-transparent hover:bg-[var(--commit-bg-emphasis)] hover:text-[var(--commit-color)]"
                    onclick={() => onStartSession('commit', nextSteps!.commitStep!)}
                  >
                    Start commit
                  </Button>
                </div>
              {/if}
            </div>
          {/if}
          <div class="selection-dock" bind:this={dockEl}></div>
        </div>
      </section>

      {#if sessionId && chatOpen}
        <aside id="note-modal-chat-pane" class="chat-pane" aria-label="Chat">
          <PooledSessionChatPane
            compact
            active={chatOpen}
            {sessionId}
            {repoDir}
            {branchId}
            {projectId}
            {repoLabel}
            {hashtagItems}
            {noteInfo}
            {onOpenSession}
            onSessionChange={handlePaneSessionChange}
            {onHashtagClick}
            onStartNewSession={startSessionFromChat}
            newSessionDisabled={startSessionFromChatDisabled}
          />
        </aside>
      {/if}
    </div>
    <Dialog.ResizeHandle
      minWidth={totalMinWidth}
      onWidthChange={handleWidthChange}
      onResizeStart={() => (resizing = true)}
      onResizeEnd={() => {
        dialogWidth.clearPreview();
        resizing = false;
      }}
      onReset={() => dialogWidth.reset()}
    />
  </Dialog.Content>
</Dialog.Root>

{#if mediaViewer}
  <MediaViewerModal
    open={true}
    src={resolveStagedMediaUrl(mediaViewer.id, mediaViewer.ext)}
    title={mediaViewer.alt}
    originImage={mediaViewer.originImage}
    gesture={mediaViewer.gesture}
    onClose={() => (mediaViewer = null)}
  />
{/if}

<DiagramViewerModal
  open={diagramViewerSvg !== null}
  svgMarkup={diagramViewerSvg}
  onClose={() => (diagramViewerSvg = null)}
/>

<style>
  /* The first row of a full-screen dialog: clear of its top inset, the
     status bar (`--dialog-safe-top`, see dialog-content). */
  .note-modal-header-grid {
    display: flex;
    align-items: stretch;
    justify-content: space-between;
    width: 100%;
    min-width: 0;
    padding-top: var(--dialog-safe-top, 0px);
  }

  /* Fixed chat column: the preferred dialog width is the note width plus
     CHAT_PANE_WIDTH. The note column absorbs resizing and viewport compression.
     Keep the second track in sync with
     CHAT_PANE_WIDTH in the script above. */
  .note-modal-header-grid.split-chat-open,
  .modal-body.split-chat-open {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 380px;
  }

  .note-header-pane {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    flex: 1;
    padding: 12px 16px;
    /* The header's ends clear a full-screen dialog's side insets, Close
       included (`--dialog-safe-left` / `--dialog-safe-right`, see
       dialog-content); the dialog is never split there. */
    padding-left: calc(16px + var(--dialog-safe-left, 0px));
  }

  .note-header-pane.split-pane {
    flex: 2 1 0;
  }

  .chat-header-pane {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
    flex: 1 1 0;
    padding: 12px;
    padding-right: calc(12px + var(--dialog-safe-right, 0px));
    border-left: 1px solid var(--border-subtle);
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }

  .note-title-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 5px;
    flex-shrink: 0;
    color: var(--note-color);
    background: var(--note-bg);
  }

  .note-header-actions,
  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .header-actions {
    padding: 12px 16px 12px 0;
    padding-right: calc(16px + var(--dialog-safe-right, 0px));
  }

  .modal-body {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }

  /* With the keyboard up on a phone held sideways only the composer's row
     fits (dialog-content's `cramped`): the chat view drops its header, as
     the session dialog does, unless the header holds the focus (the search
     field). It is back once the keyboard closes. */
  :global([data-cramped]) > :global(.note-modal-chat-header:not(:focus-within)) {
    display: none;
  }

  /* ...so the top inset goes to the dialog itself, ahead of the chat. */
  :global([data-cramped]:has(> .note-modal-chat-header:not(:focus-within))) {
    padding-top: var(--dialog-safe-top, 0px);
  }

  .note-pane {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
    flex-direction: column;
  }

  .split-chat-open .note-pane {
    flex: 2 1 0;
  }

  .chat-only .note-pane {
    display: none;
  }

  .chat-pane {
    display: flex;
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
    border-left: 1px solid var(--border-subtle);
    background: var(--bg-primary);
  }

  .chat-only .chat-pane {
    border-left: none;
  }

  /* The note's scroll area; the pane's bottom slot (`.note-bottom`) follows
     it. */
  .note-scroll {
    display: flex;
    flex: 1;
    min-height: 0;
    flex-direction: column;
  }

  /* The note spans a full-screen dialog's width, so a swipe over the side
     insets still scrolls it; its text clears them. While the Reply button
     is docked and reaches up over the note's bottom edge the text clears
     that too (`--selection-dock-overlap`, measured above), so a selection
     ending in the last lines can scroll out from behind it. */
  /* Positioned for the selection's Reply button and for what the diff
     draws, which scrolls with the text: its markers, and the rectangle
     behind each change (NoteDiffBlocks), which is painted under the text
     and so needs the scroll area to be a stacking context. The markers
     stand in the right padding, as far in as the text's inset. A rectangle
     starts half-way across the left padding, which is where the band from
     the previous version lands when that is open
     (`--note-diff-blocks-reach`, set above), and ends at its marker's left
     edge: the markers are 8px from the edge and 10px wide. Both hold still
     when a wide table makes the note scroll sideways, for which they are
     told the left padding (`--note-diff-scroll-padding`). */
  .modal-content {
    --note-diff-markers-inset: var(--dialog-safe-right, 0px);
    --note-diff-scroll-padding: calc(24px + var(--dialog-safe-left, 0px));
    --note-diff-blocks-left: calc(
      var(--note-diff-blocks-reach, 12px) + var(--dialog-safe-left, 0px)
    );
    --note-diff-blocks-right: calc(18px + var(--dialog-safe-right, 0px));
    position: relative;
    isolation: isolate;
    flex: 1;
    overflow-y: auto;
    padding: 24px;
    padding-left: calc(24px + var(--dialog-safe-left, 0px));
    padding-right: calc(24px + var(--dialog-safe-right, 0px));
    padding-bottom: calc(24px + var(--selection-dock-overlap, 0px));
    min-height: 0;
    background: var(--bg-primary);
  }

  /* On a touch screen a swipe past either end of the note stops there
     instead of moving whatever scrolls behind the dialog. */
  @media (pointer: coarse) {
    .modal-content {
      overscroll-behavior: contain;
    }
  }

  /* With no bottom bar below it the note ends the dialog: it scrolls behind
     the home indicator of a full-screen dialog, and its last line can still
     scroll clear of it (`--dialog-safe-bottom`, see dialog-content). The
     docked button clears the indicator itself, so its overlap stands in for
     the inset while it shows. */
  .modal-content.ends-pane {
    padding-bottom: calc(
      24px + max(var(--selection-dock-overlap, 0px), var(--dialog-safe-bottom, 0px))
    );
  }

  /* Beside its previous version the note has room under it, so its last
     change can reach the line the two panes are kept level on, a third of
     the way down (NotePreviousPane has the same). It is not at the dialog's
     left edge there, so it has no inset to clear on that side: its left
     padding is a fixed width, the first half of which the diff's
     connectors cross to reach the rectangles behind its changes
     (NoteDiffConnectors). */
  .modal-body.split-diff-open .modal-content {
    --note-diff-blocks-left: var(--note-diff-blocks-reach, 12px);
    --note-diff-scroll-padding: 24px;
    padding-bottom: 60vh;
    padding-left: 24px;
  }

  .modal-content::-webkit-scrollbar {
    width: 6px;
  }

  .modal-content::-webkit-scrollbar-track {
    background: transparent;
  }

  .modal-content::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 3px;
  }

  .modal-content::-webkit-scrollbar-thumb:hover {
    background: var(--scrollbar-thumb-hover-transparent);
  }

  .empty-note {
    margin: 0;
    color: var(--text-faint);
    font-size: var(--size-sm);
    font-style: italic;
    text-align: center;
    padding: 24px 0;
  }

  /* Markdown styles — matches SessionModal */
  .markdown-content {
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.7;
  }

  .markdown-content :global(p) {
    margin: 0 0 0.75em 0;
  }

  .markdown-content :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3),
  .markdown-content :global(h4) {
    margin: 1em 0 0.5em 0;
    font-weight: 600;
    line-height: 1.3;
  }

  .markdown-content :global(h1:first-child),
  .markdown-content :global(h2:first-child),
  .markdown-content :global(h3:first-child),
  .markdown-content :global(h4:first-child) {
    margin-top: 0;
  }

  /* A section link (`[text](#heading-slug)`) scrolls its heading, or the
     anchor an author named (`<a name="…">`), to the top of the note and
     moves focus there (sectionLinks.ts). The margin is the room left above
     it: nothing is stuck over the scroll box's top edge, so it is breathing
     space only. The ring shows where a keyboard landed; an anchor with
     nothing in it has nothing to ring. */
  .markdown-content :global(:is(h1, h2, h3, h4, h5, h6, a[name])) {
    scroll-margin-top: 16px;
  }

  .markdown-content :global(:is(h1, h2, h3, h4, h5, h6, a[name]):focus-visible) {
    outline: 2px solid var(--ui-accent);
    outline-offset: 4px;
    border-radius: 2px;
  }

  .markdown-content :global(a[name]:empty:focus-visible) {
    outline: none;
  }

  .markdown-content :global(h1) {
    font-size: 1.25em;
  }
  .markdown-content :global(h2) {
    font-size: 1.15em;
  }
  .markdown-content :global(h3) {
    font-size: 1.05em;
  }

  .markdown-content :global(ul),
  .markdown-content :global(ol) {
    margin: 0.5em 0;
    padding-left: 1.5em;
  }

  .markdown-content :global(ul) {
    list-style-type: disc;
  }

  .markdown-content :global(ol) {
    list-style-type: decimal;
  }

  .markdown-content :global(ul ul) {
    list-style-type: circle;
  }

  .markdown-content :global(ul ul ul) {
    list-style-type: square;
  }

  .markdown-content :global(li) {
    margin: 0.25em 0;
  }

  .markdown-content :global(code) {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: 0.9em;
    background: var(--bg-elevated, var(--bg-hover));
    padding: 0.15em 0.35em;
    border-radius: 3px;
  }

  .markdown-content :global(pre) {
    margin: 0.75em 0;
    padding: 0.75em;
    background: var(--bg-elevated, var(--bg-hover));
    border-radius: 6px;
    overflow-x: auto;
  }

  .markdown-content :global(pre code) {
    background: none;
    padding: 0;
    font-size: 0.85em;
  }

  .markdown-content :global(blockquote) {
    margin: 0.5em 0;
    padding-left: 0.75em;
    border-left: 3px solid var(--border-muted);
    color: var(--text-muted);
  }

  .markdown-content :global(a) {
    color: var(--ui-accent);
    text-decoration: none;
  }

  @media (hover: hover) {
    .markdown-content :global(a:hover) {
      text-decoration: underline;
    }
  }

  .markdown-content :global(strong) {
    font-weight: 600;
  }

  .markdown-content :global(hr) {
    margin: 1em 0;
    border: none;
    border-top: 1px solid var(--border-subtle);
  }

  .markdown-content :global(table) {
    border-collapse: collapse;
    width: 100%;
    margin: 0.75em 0;
  }

  .markdown-content :global(th),
  .markdown-content :global(td) {
    border: 1px solid var(--border-subtle);
    padding: 6px 12px;
    text-align: left;
  }

  .markdown-content :global(th) {
    background: var(--bg-primary);
    font-weight: 600;
  }

  .next-steps {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 16px;
    padding-bottom: calc(12px + var(--dialog-safe-bottom, 0px));
    padding-left: calc(16px + var(--dialog-safe-left, 0px));
    padding-right: calc(16px + var(--dialog-safe-right, 0px));
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    flex-shrink: 0;
  }

  .next-step-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    background: var(--bg-elevated);
    border-radius: 8px;
  }

  .next-step-prompt {
    flex: 1;
    font-size: var(--size-sm);
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .chat-info-row {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
  }

  /* The pane's bottom slot. Its height in the flow is the bottom bar's, or
     nothing; the docked Reply button is laid over it rather than in the
     flow, so the button's arrival takes no height from the note above (what
     it covers is the bar, hidden meanwhile, and the note's content is
     padded by whatever reaches past the slot, and keeps scrolling under
     it). The note's scroll box keeps its height, and so its offset, while
     the finger that made the selection is still on the text. */
  .note-bottom {
    position: relative;
    flex-shrink: 0;
  }

  /* Holds the docked Reply button on a phone; nothing otherwise. Stands on
     the slot's bottom edge and covers at least the slot, so none of the bar
     shows past it; the chrome fills what the row leaves. Always the end of
     the pane, it clears the home indicator. The box is there whether or not
     the row is, laid over the bar at the slot's size, so it takes no
     presses itself: empty, a press on it is for the bar's buttons under it;
     docked, the row (`.selection-reply-dock`) takes its own. */
  .selection-dock {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    min-height: 100%;
    pointer-events: none;
  }

  .selection-dock:not(:empty) {
    padding-bottom: var(--dialog-safe-bottom, 0px);
    background: var(--bg-chrome);
  }

  /* The bar under the docked button keeps its box, and nothing else: not
     painted, not pressable, not read. */
  .next-steps.is-covered {
    visibility: hidden;
  }

  /* One button on a phone fills the bar, a finger-sized row. */
  @media (pointer: coarse) {
    .chat-info-row > :global(.reply-draft-button:only-child) {
      width: 100%;
    }
  }
</style>
