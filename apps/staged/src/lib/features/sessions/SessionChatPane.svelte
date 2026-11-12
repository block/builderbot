<!--
  SessionChatPane.svelte — View an AI session (live or historical)

  The single session viewer used everywhere in the app. Shows the message
  transcript with user messages as right-aligned chat bubbles, assistant
  messages as plain markdown, and tool calls as collapsible cards with
  argument previews.

  Features:
  - Text input always available at the bottom (Cmd/Ctrl+Enter sends, Enter adds a newline)
  - Send button when idle, Queue button when running; Stop lives on the Thinking row
  - While a pipeline runs with an empty transcript, the composer is replaced by
    a labeled Stop button; once messages exist, Stop lives on the Thinking row
    and the composer queues follow-ups as usual
  - Message queue: sending while running persists a follow-up for backend drain
  - Live send: when the backend reports the running session can take a message
    now (`get_session_live_delivery`), the composer offers "Send" beside
    "Queue"; "Send" goes through `send_session_message_now`, the live path the
    `continue_repo_session` MCP tool uses (steer, else interrupt-and-reprompt);
    a session that stopped meanwhile falls back to the idle resume path
  - Copy button on every message
  - Tool calls show name + args preview; expand to see output
  - Fixed-size modal with proper scrolling

  For running sessions, polls the DB incrementally:
  - Re-fetches the last known message (it may have grown from streaming)
  - Fetches any new messages after it
  - Stops polling when status leaves "running"

  Props:
    sessionId — the session to display
    active    — whether the pane should load/poll
-->
<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from 'svelte';
  import { toast } from 'svelte-sonner';
  import { slide } from 'svelte/transition';
  import X from '@lucide/svelte/icons/x';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import Info from '@lucide/svelte/icons/info';
  import CircleStop from '@lucide/svelte/icons/circle-stop';
  import CloudCheck from '@lucide/svelte/icons/cloud-check';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Zap from '@lucide/svelte/icons/zap';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import BookOpen from '@lucide/svelte/icons/book-open';
  import FileText from '@lucide/svelte/icons/file-text';
  import Spinner from '../../shared/Spinner.svelte';
  import { isResumableReason } from '../../types';
  import type {
    AcpConfigSelection,
    AcpConfigValueSelection,
    Session,
    SessionBackgroundHoldPayload,
    SessionMessage,
    SessionReconcileEvent,
    SessionStatus,
    QueuedSessionMessage,
    HashtagItem,
    ProjectRepo,
    SessionStatusPayload,
  } from '../../types';
  import {
    buildNoteFollowupMessage,
    cancelSession,
    createImage,
    createImageFromData,
    deleteImage,
    discoverAcpConfig,
    getAttachmentThumbnail,
    getSession,
    getSessionAcpMetadataMessages,
    getSessionBackgroundHold,
    getSessionLiveDelivery,
    getLatestSessionMessages,
    getOlderSessionMessages,
    handleExternalLinkClick,
    deleteQueuedSessionMessage,
    listQueuedSessionMessages,
    pollSession,
    queueSessionMessage,
    reconcileSession,
    resumeSession,
    sendQueuedSessionMessage,
    sendSessionMessageNow,
    stopSessionAsyncTask,
    type AcpConfigDiscovery,
    type AcpConfigSelector,
  } from '../../api/commands';
  import { listenToEvent, writeClipboardText, type UnlistenFn } from '../../transport';
  import { openSettings } from '../layout/navigation.svelte';
  import AgentLoginPrompt from '../doctor/AgentLoginPrompt.svelte';
  import { isAuthenticationError } from './authRecovery';
  import { createSessionLoginController } from './sessionLogin.svelte';
  import AcpFixedConfigPicker from '../agents/AcpFixedConfigPicker.svelte';
  import { agentState } from '../agents/agent.svelte';
  import {
    buildAcpConfigSelection,
    defaultSelectorValue,
    selectorHasValue,
  } from '../agents/acpConfigSelection';
  import {
    preferences,
    setAcpConfigPref,
    setComposerSendMode,
  } from '../settings/preferences.svelte';
  import ChatComposer, { composerControlClass } from './ChatComposer.svelte';
  import ComposerError from './ComposerError.svelte';
  import {
    composerSendControl,
    composerSessionBlock,
    type ComposerNewSessionDraft,
    type ComposerNewSessionKind,
  } from './composerSendControl';
  import DisconnectedSessionAlert from './DisconnectedSessionAlert.svelte';
  import { disconnectedSessionsStore } from '../../stores/disconnectedSessions.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { QUEUED_FOR_TRANSFER_LABEL } from '../projects/projectTransfer';
  import { isReconcileCancelNotice } from '../../shared/disconnectedSession';
  import {
    LIVE_DELIVERY_UNKNOWN,
    liveDeliveryKnowledge,
    shouldAskLiveDelivery,
    type LiveDeliveryKnowledge,
  } from './liveDelivery';
  import {
    backgroundHoldTaskRows,
    isTaskHeld,
    knownSessionStatus,
    liveActivityRow,
    nextBackgroundHold,
    pruneStoppingTaskIds,
    pruneTaskStopNotices,
    statusEventSupersededLoad,
    type SessionBackgroundHold,
  } from './backgroundHold';
  import { mergeSinceMessages, pollShouldStop, sessionPollRequest } from './messagePoll';
  import { retainInlineToolBodies } from './toolCallBodies';
  import {
    applyLatestWindow,
    LOAD_OLDER_THRESHOLD_PX,
    metadataRowsForWindow,
    pageRequestLimit,
    prependOlderMessages,
    TRANSCRIPT_PAGE_ROWS,
    transcriptPageFromRows,
    trimLatestWindow,
  } from './transcriptWindow';
  import {
    buildBranchHashtagItems,
    findHashtagItemForReference,
    renderHashtagTokens as renderHashtagTokensShared,
  } from './hashtagItems';
  import * as Alert from '$lib/components/ui/alert';
  import { Button } from '$lib/components/ui/button';
  import { subscribeDragDrop } from '../branches/dragDrop';
  import {
    isImageAttachmentFile,
    isMaybeTextFile,
    insertFilePathsAtCursor,
  } from '../branches/branchCardHelpers';
  import {
    hasXmlBlocks,
    reconnectedOutcomeMessage,
    newestUserMessageId,
    sessionEndMessage,
    sessionErrorMessage,
    XML_BLOCK_TAGS,
  } from './sessionModalHelpers';
  import AttachmentThumb from './AttachmentThumb.svelte';
  import { localPreview, type AttachmentPreview } from './attachmentPreview';
  import { fileToBase64 } from './attachmentData';
  import {
    attachBasename,
    attachErrorReason,
    formatAttachErrors,
    isAttachableClipboardType,
    type AttachRejection,
  } from './attachErrors';
  import {
    buildAcpTranscriptGroups,
    formatJson,
    groupRichToolsByVerb,
    latestAvailableCommands,
    latestPlan,
    stabilizeAcpTranscriptGroups,
    transcriptGroupKey,
    type AcpCommand,
    type AcpTranscriptEvent,
    type AcpTranscriptGroup,
  } from './acpTranscript';
  import PlanCard from './PlanCard.svelte';
  import { viewport } from '../../shared/viewport.svelte';
  import { autofocusOnOpen } from '../../shared/focusAfterDialogEnter';
  import { observeSizeChanges } from '../../shared/observeSizeChanges';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import {
    displayRootKey,
    normalizeDisplayRoots,
    resolveDisplayRoots,
    type DisplayRootInput,
  } from './pathDisplayRoots';
  import PipelineSteps from './PipelineSteps.svelte';
  import ToolCallCard from './tool-calls/ToolCallCard.svelte';
  import ToolCallHeader from './tool-calls/ToolCallHeader.svelte';
  import { highlightMatches, clearHighlights, scrollToMatch } from '../../shared/textHighlight';
  import { revealWithin } from '../../shared/revealWithin';
  import '../../shared/markdown/diagramStyles.css';
  import {
    extractMarkdownDiagramFences,
    fencedDiagramMarkdown,
  } from '../../shared/markdown/diagramFormats';
  import MediaViewerModal from '../../shared/markdown/MediaViewerModal.svelte';
  import { getMarkdownMediaFromEvent, type MarkdownMedia } from '../../shared/markdown/mediaViewer';
  import { resolveStagedMediaUrl } from '../../shared/markdown/mediaUrl';
  import '../../shared/markdown/mediaStyles.css';
  import DiagramViewerModal from '../../shared/markdown/DiagramViewerModal.svelte';
  import {
    getMarkdownDiagramSvgMarkup,
    isMarkdownDiagramActivationKey,
  } from '../../shared/markdown/diagramViewer';
  import { renderMarkdown as renderSharedMarkdown } from '../../shared/markdown/renderMarkdown';
  import { loadPikchrRenderer, type PikchrRenderer } from '../../shared/markdown/pikchrRendering';
  import { getNoteFollowupLabel, noteActivityLabel, type LinkedNoteContext } from './noteFreshness';
  import {
    offerUnstartedDraftBox,
    otherUnstartedDraftBox,
    restoreUnstartedAttachments,
    restoreUnstartedDraft,
    unstartedDraftMessage,
    unstartedDraftToast,
  } from './unstartedDraft';
  import {
    loadReplyDraft as loadStoredReplyDraft,
    amendStoredReplyDraft,
    restoreQuoteInReplyDraft,
    registerReplyComposer,
    replyDraftKey,
    replyDraftWriter,
  } from './replyDrafts';
  import {
    appendDraftQuote,
    cloneComposerDraft,
    composerDraftContent,
    composerDraftsEqual,
    emptyComposerDraft,
    hasQuotedParts,
    isComposerDraftEmpty,
    parseQuotedMessage,
    restoreDraftQuote,
    serializeComposerDraft,
    setDraftQuoteReply,
    takeDraftQuote,
    type ComposerDraft,
    type DraftQuote,
    type RemovedDraftQuote,
  } from './composerDraft';
  import QuoteExcerpt from './QuoteExcerpt.svelte';
  import NoteActivityCard from './NoteActivityCard.svelte';
  import { splitAtNoteIndicator } from './noteIndicators';
  import type { HashtagClickInfo } from '../references/referenceHistory.svelte';
  import { latestAcpConfigDiscoveryFromMetadata } from './acpConfigMetadata';
  import {
    applyParkedScroll,
    createDeferredRestore,
    reopenAfterRevalidate,
    reopenMode,
    type ParkedScroll,
    type PollOutcome,
    type ReopenMode,
  } from './paneReopen';
  import { captureScrollAnchor, restoreScrollAnchor, type ScrollAnchor } from './scrollAnchor';
  import { createIntroGate } from './transcriptIntro';
  import StartedByLabel from './StartedByLabel.svelte';

  interface Props {
    active: boolean;
    sessionId: string;
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
    /** Linked note context for note-related chat actions. */
    noteInfo?: LinkedNoteContext | null;
    onOpenSession?: (sessionId: string) => void;
    onHashtagClick?: (click: HashtagClickInfo) => void;
    onSessionChange?: (session: Session | null) => void;
    onSearchStateChange?: (state: { matchCount: number; currentIndex: number }) => void;
    /**
     * Start a new session from the composer's draft instead of sending it
     * (the note dialog's "New note" / "New commit"). Resolves whether the
     * session started; on `false` the host has reported why. The pane takes
     * the draft out of the box before calling, as a send does, and on
     * `false` (or a throw) puts it back above anything typed since and says
     * so, by toast when it is no longer on screen; the host may close the
     * dialog (parking this pane) as soon as it is called. When absent the
     * composer does not offer the actions.
     */
    onStartNewSession?: (
      kind: ComposerNewSessionKind,
      draft: ComposerNewSessionDraft
    ) => Promise<boolean>;
    /** The host cannot start a session right now: the actions show disabled. */
    newSessionDisabled?: boolean;
    compact?: boolean;
  }

  type SendMessageTarget = {
    sessionId: string;
    branchId: string | null;
  };

  let {
    active,
    sessionId,
    repoDir,
    branchId,
    projectId,
    repoLabel = null,
    hashtagItems: providedHashtagItems,
    noteInfo,
    onOpenSession,
    onHashtagClick,
    onSessionChange,
    onSearchStateChange,
    onStartNewSession,
    newSessionDisabled = false,
    compact = false,
  }: Props = $props();

  // =========================================================================
  // State
  // =========================================================================

  let session = $state<Session | null>(null);
  /**
   * The loaded suffix of the visible transcript: the newest page on open,
   * extended backwards by `loadOlderMessages` as the user scrolls up and
   * forwards by the poll. See `transcriptWindow.ts`.
   */
  let messages = $state<SessionMessage[]>([]);
  /** Whether visible rows older than `messages[0]` exist on the server. */
  let hasOlderMessages = $state(false);
  let loadingOlder = $state(false);
  let acpMetadataMessages = $state<SessionMessage[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let cancelling = $state(false);
  /**
   * A "Check now" the backend hasn't answered yet: pending while the
   * session's reconcile state is still the one it was clicked against (every
   * `session-reconcile-changed` replaces it), or until the timeout. Raw so the
   * identity comparison sees the store's own object, not a proxy of it.
   */
  let checkNowRequest = $state.raw<{
    sessionId: string;
    stateAtRequest: SessionReconcileEvent | null;
  } | null>(null);
  let checkNowTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * The disconnect this pane last saw, so a turn reconcile settled can say so
   * instead of ending silently: the session, and its newest user message at
   * the time — the prompt of the turn that was in flight. The note speaks for
   * that turn only; once a newer prompt shows up (sent here, from another
   * client, or drained from the queue) the session has moved on.
   */
  let observedDisconnect = $state<{ sessionId: string; userMessageId: number | null } | null>(null);
  let messagesEl: HTMLDivElement;
  let inputEl: HTMLElement | null = $state(null);
  let composerView = $state<ReturnType<typeof ChatComposer>>();
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollInFlight = false;
  /** A terminal status arrived while a tick was in flight; poll once more
   *  when that tick settles. */
  let finalPollPending = false;
  let unlistenStatus: UnlistenFn | null = null;
  let unlistenBackgroundHold: UnlistenFn | null = null;
  let statusEventVersion = 0;
  let closed = false;
  /** Latest background hold reported for this session; null when not holding. */
  let backgroundHold = $state<SessionBackgroundHold | null>(null);
  /**
   * What the backend has said about this running session taking a message
   * now (owned by this process, agent connection up). Asked on each poll tick
   * until the answer is settled: a yes, or a no that cannot change during this
   * run (another Staged instance owns the row). A retryable no — the
   * connection comes up a few seconds into a run — is asked again. Forgotten
   * whenever the status leaves `running` or the pane changes session.
   */
  let liveDelivery = $state<LiveDeliveryKnowledge>(LIVE_DELIVERY_UNKNOWN);
  let liveDeliveryInFlight = false;
  /**
   * Counts hold reports received by event, so the mount-time snapshot can tell
   * whether a newer report landed while it was in flight.
   */
  let backgroundHoldReportVersion = 0;
  /**
   * Session whose `session.status` describes *this* open, rather than being
   * left over from the last time the pane showed it — see `knownSessionStatus`.
   * Cleared when a load starts, set when one resolves or a status event lands.
   */
  let statusLoadedForSessionId: string | null = null;
  /** Per-task stops in flight, so each row disables its own button only. */
  let stoppingTaskIds = $state<Set<string>>(new Set());
  /**
   * What a per-task stop answered, keyed by task id and rendered on that task's
   * own row. Only outcomes the user can't see for themselves land here — see
   * `noteTaskStopOutcome`.
   */
  let taskStopNotices = $state<Map<string, string>>(new Map());
  const login = createSessionLoginController({
    getActive: () => active,
    getSessionId: () => sessionId,
    getSession: () => session,
  });

  /** The draft's free text after its last quote — the composer's main input. */
  let inputText = $state('');
  /** The draft's quote blocks, each with the free text before it (see composerDraft.ts). */
  let draftQuotes = $state<DraftQuote[]>([]);
  let queuedMessages = $state<QueuedSessionMessage[]>([]);
  let queueActionIds = $state<Set<string>>(new Set());
  let copiedId = $state<number | string | null>(null);
  let expandedTools = $state<Set<string>>(new Set());
  let displayRoots = $state<string[]>([]);
  let currentDisplayRootKey = '';

  // Reply drafts persist through the writer every pane shares (see
  // replyDrafts.ts), so a quote added from a note while no composer shows the
  // draft can amend the stored copy without racing this pane's debounced save.
  // $state so the save effect re-runs when a finished load opens the gate —
  // otherwise a keystroke typed during the load window is never scheduled.
  let loadedDraftSessionId = $state<string | null>(null);
  let draftLoadGeneration = 0;
  // The newest draft load (it never rejects), for `putBackUnstartedDraft` to
  // wait out before it decides whether the box or the store holds the draft.
  let replyDraftLoad: Promise<void> = Promise.resolve();
  // Unmounted (a transient or evicted pane): nothing will show this box again.
  let destroyed = false;
  // The toast `putBackUnstartedDraft` raised while this pane was parked, and
  // the session it was about: taken down when the pane next shows that
  // session, where the composer's own line says the same.
  let unstartedDraftToastShown: { toastId: string | number; sessionId: string } | null = null;
  // One-shot: set when a failed load re-opens the save gate while the composer
  // may still hold the previously shown session's text; the save effect skips
  // exactly that run so inherited text isn't filed under this session's key.
  let suppressNextReplySave = false;
  /**
   * Edits a note made to this session's draft (a quote added, its reply
   * typed, a removed block put back) while the draft's load was still in
   * flight: they are applied once the stored draft has landed, rather than
   * being replaced by it, and what was to follow each in the composer (the
   * caret into the block, or the field scrolled to it) runs then too, once
   * the block is rendered. Keyed to the session they were made for.
   */
  let pendingDraftEdits: {
    sessionId: string;
    edits: Array<{ edit: (draft: ComposerDraft) => ComposerDraft; then?: () => void }>;
  } | null = null;
  /**
   * Undo toasts for removed quote blocks still on screen (svelte-sonner
   * keeps one up for seconds). A send the backend accepts clears the draft
   * they were taken from, and a switch shows another session's; either
   * dismisses them, so Undo can't put a block into a draft it never belonged
   * to. `draftEpoch` counts the sends, for a click that lands as the toast
   * is leaving.
   */
  let quoteUndoToasts: Array<{ toastId: string | number; sessionId: string }> = [];
  let draftEpoch = 0;

  /** The composer's draft as one value (live `$state`; clone before keeping it). */
  function currentDraft(): ComposerDraft {
    return { quotes: draftQuotes, text: inputText };
  }

  function setDraft(draft: ComposerDraft) {
    draftQuotes = draft.quotes;
    inputText = draft.text;
  }

  /**
   * Restore the cached reply draft for `id` into the composer.
   *
   * Flushes the writer before reading: bouncing between sessions faster than
   * the write debounce (A → B → A) leaves A's newest keystrokes pending, and a
   * direct store read would restore the stale value — which the save effect
   * below would then write back, superseding the pending newer text.
   */
  async function loadReplyDraft(
    id: string,
    generation: number,
    draftWhenLoadStarted: ComposerDraft
  ) {
    try {
      await replyDraftWriter.flush();
      const draft = await loadStoredReplyDraft(id);
      if (generation !== draftLoadGeneration || !active || sessionId !== id) return;
      if (composerDraftsEqual(cloneComposerDraft(currentDraft()), draftWhenLoadStarted)) {
        setDraft(draft);
      }
      loadedDraftSessionId = id;
      applyPendingDraftEdits(id);
    } catch (err) {
      console.error('[SessionChatPane] Failed to load reply draft:', err);
      // Leave the composer as the user left it, but re-enable saving so
      // further keystrokes still reach the store. Re-enabling fires the save
      // effect once immediately: when the composer still holds exactly the
      // draft from before this load began, that draft was inherited from the
      // previously shown session (a switch A → B where B's load failed), and
      // letting that run through would overwrite B's stored draft with A's
      // text. Suppress it — text typed during the load window was written
      // while this session was displayed, so it still saves.
      if (generation === draftLoadGeneration && active && sessionId === id) {
        suppressNextReplySave = composerDraftsEqual(
          cloneComposerDraft(currentDraft()),
          draftWhenLoadStarted
        );
        loadedDraftSessionId = id;
        applyPendingDraftEdits(id);
      }
    }
  }

  /**
   * A note edited this session's draft while the composer shows it (see
   * replyDrafts.ts): applied to the composer, or held until its draft has
   * loaded. `then` follows the edit into the composer — at once, or after
   * the held edits land — so a caret or scroll meant for a block the edit
   * adds finds it rendered rather than running against the loading draft.
   * It is dropped with the edits that go to the stored draft instead when
   * the pane moves on, there being no composer to act in.
   */
  function receiveDraftEdit(
    id: string,
    edit: (draft: ComposerDraft) => ComposerDraft,
    then?: () => void
  ) {
    if (loadedDraftSessionId !== id) {
      if (pendingDraftEdits?.sessionId !== id) pendingDraftEdits = { sessionId: id, edits: [] };
      pendingDraftEdits.edits.push({ edit, then });
      return;
    }
    // Quoting is a draft edit: the last failure line has been read.
    sendError = null;
    setDraft(edit(currentDraft()));
    then?.();
  }

  function applyPendingDraftEdits(id: string) {
    const pending = pendingDraftEdits;
    pendingDraftEdits = null;
    if (pending?.sessionId !== id) return;
    setDraft(pending.edits.reduce((draft, { edit }) => edit(draft), currentDraft()));
    for (const { then } of pending.edits) then?.();
  }

  /** Put the caret in a quote block's reply, once the block is rendered. */
  function focusDraftQuoteReply(quoteId: string) {
    tick().then(() => {
      if (!closed && active) composerView?.focusQuoteReply(quoteId);
    });
  }

  /**
   * Run `then` once this session's draft is in the composer: at once, or
   * behind the held edits when the draft is still loading (the block a note
   * just added is among them, and isn't rendered yet).
   */
  function whenDraftLoaded(id: string, then: () => void) {
    if (loadedDraftSessionId === id) {
      then();
      return;
    }
    if (pendingDraftEdits?.sessionId !== id) pendingDraftEdits = { sessionId: id, edits: [] };
    pendingDraftEdits.edits.push({ edit: (draft) => draft, then });
  }

  /** Scroll the composer's field to a quote block, once it is rendered, leaving focus where it is. */
  function revealDraftQuote(quoteId: string) {
    tick().then(() => {
      if (!closed && active) composerView?.revealQuote(quoteId);
    });
  }

  /**
   * Take a quote block out of the draft. One with a reply typed under it is
   * offered back in a toast (there is no other undo for a block): Undo puts
   * it back where it was, reply and lead-in text included, and — with a
   * pointer, not a touch screen — the caret in its reply; on a touch screen
   * the field is scrolled to show it instead. An unanswered block just goes.
   */
  function removeQuote(id: string) {
    const taken = takeDraftQuote(currentDraft(), id);
    if (!taken) return;
    sendError = null;
    setDraft(taken.draft);
    if (taken.removed.quote.reply.trim() !== '') offerQuoteUndo(sessionId, taken.removed);
  }

  function offerQuoteUndo(sid: string | null, removed: RemovedDraftQuote) {
    if (!sid) return;
    const epoch = draftEpoch;
    const forget = () => {
      quoteUndoToasts = quoteUndoToasts.filter((entry) => entry.toastId !== toastId);
    };
    const toastId = toast('Quote removed from your reply', {
      onDismiss: forget,
      onAutoClose: forget,
      action: {
        label: 'Undo',
        onClick: () => {
          // The toast goes with the press (svelte-sonner deletes it without
          // `onDismiss`), so it is forgotten here.
          forget();
          // The draft the block was taken from has been sent since.
          if (epoch !== draftEpoch) return;
          if (active && sessionId === sid) {
            // The same way in as a quote a note adds: applied to the
            // composer, or held until this session's draft has loaded, so a
            // load in flight can't land over it.
            // The caret goes back into the reply for a keyboard user; with
            // a touch screen that would raise the on-screen keyboard over
            // the restored draft, which the user hasn't asked to type in,
            // so there the field is only scrolled to the block, which in a
            // capped field that has scrolled on may otherwise come back
            // out of view. Either follows the block into the composer, so
            // a restore held for the load isn't looked for before it lands.
            receiveDraftEdit(
              sid,
              (draft) => restoreDraftQuote(draft, removed),
              () => {
                if (!viewport.hasCoarsePointer) focusDraftQuoteReply(removed.quote.id);
                else revealDraftQuote(removed.quote.id);
              }
            );
            return;
          }
          // This pane has moved on: another pane showing the draft takes it,
          // or else the stored draft.
          restoreQuoteInReplyDraft(sid, removed).catch((err) => {
            console.error('[SessionChatPane] Failed to restore the quote:', err);
            toast.error('Could not restore the quote');
          });
        },
      },
    });
    quoteUndoToasts = [...quoteUndoToasts, { toastId, sessionId: sid }];
  }

  /** Take down the Undo toasts whose draft is gone or off screen (all, by default). */
  function dismissQuoteUndo(keep: (sessionId: string) => boolean = () => false) {
    const going = quoteUndoToasts.filter((entry) => !keep(entry.sessionId));
    if (going.length === 0) return;
    quoteUndoToasts = quoteUndoToasts.filter((entry) => keep(entry.sessionId));
    for (const entry of going) toast.dismiss(entry.toastId);
  }

  /**
   * The draft has gone with a message the backend accepted: a block removed
   * from it has nothing to go back into. Not before — a send that fails puts
   * the draft back in the box, and the block's Undo should still hold then.
   * (Pressed while the send is in flight, Undo puts the block into the
   * emptied box, where it is a new draft.)
   */
  function draftSent() {
    draftEpoch += 1;
    dismissQuoteUndo();
  }

  let discoveredFollowupConfig = $state<AcpConfigDiscovery | null>(null);
  let modelSpecificFollowupConfig = $state<AcpConfigDiscovery | null>(null);
  let modelSpecificFollowupModelValue = $state<string | null>(null);
  let modelSpecificFollowupSessionId = $state<string | null>(null);
  let followupConfigLoading = $state(false);
  let followupConfigError = $state<string | null>(null);
  let selectedFollowupModelValue = $state<string | null>(null);
  let selectedFollowupEffortValue = $state<string | null>(null);
  let followupModelTouchedSessionId = $state<string | null>(null);
  let followupEffortTouchedSessionId = $state<string | null>(null);
  let followupModelStateKey = $state<string | null>(null);
  let followupEffortStateKey = $state<string | null>(null);
  // A touched selection belongs to the session it was made in. The pane is
  // reused across sessions and effort ids are near-universal within a
  // provider, so a bare boolean would let an explicit choice from one
  // session survive the state-key reset and carry into the next.
  let followupModelTouched = $derived(
    followupModelTouchedSessionId !== null && followupModelTouchedSessionId === session?.id
  );
  let followupEffortTouched = $derived(
    followupEffortTouchedSessionId !== null && followupEffortTouchedSessionId === session?.id
  );
  let followupDiscoveryRun = 0;

  let isLive = $derived(session?.status === 'running');
  let isDisconnected = $derived(session?.status === 'disconnected');
  /**
   * The session's project is being transferred. A reply still goes: the
   * backend holds it as a queued follow-up and sends it once the project has
   * moved, so the queue shows it as waiting for the transfer and the manual
   * Send is withheld meanwhile.
   */
  let transferring = $derived(!!projectId && projectsDataStore.isProjectTransferring(projectId));
  /** How often an open pane re-fetches a `disconnected` session, in case it
   *  missed the status event reconcile ends it with. */
  const DISCONNECTED_REFETCH_MS = 15_000;
  /** Where the backend's attempts to catch up with this session stand. */
  let reconcileState = $derived(
    session ? disconnectedSessionsStore.getReconcileState(session.id) : null
  );
  let checkNowPending = $derived(
    !!checkNowRequest &&
      checkNowRequest.sessionId === session?.id &&
      checkNowRequest.stateAtRequest === reconcileState
  );
  let reconnectedNote = $derived(
    session &&
      observedDisconnect?.sessionId === session.id &&
      observedDisconnect.userMessageId === newestUserMessageId(messages)
      ? reconnectedOutcomeMessage(session)
      : null
  );

  // Remember a disconnect seen here, from the load, a status event or a poll.
  $effect(() => {
    if (session?.status !== 'disconnected') return;
    const sessionId = session.id;
    const userMessageId = newestUserMessageId(messages);
    if (
      observedDisconnect?.sessionId !== sessionId ||
      observedDisconnect.userMessageId !== userMessageId
    ) {
      observedDisconnect = { sessionId, userMessageId };
    }
  });
  let canSendLive = $derived(isLive && liveDelivery.supported);
  // The one send control: `handleSend` acts on it and `ChatComposer` draws
  // it, so what the button shows and what pressing it does cannot diverge.
  let sendControl = $derived(
    composerSendControl({
      isLive,
      canSendLive,
      mode: preferences.composerSendMode,
      canStartNewSession: onStartNewSession !== undefined,
    })
  );
  // Holding is a sub-state of running, so the session stays live and only the
  // activity row changes: the wait plus its live task count, and a Stop that
  // says what it stops.
  let activityRow = $derived(liveActivityRow(isLive ? backgroundHold : null));
  // One row per named live task (typed asyncTasks only — raw mode names
  // nothing and keeps the bare count), each stoppable on its own.
  let holdTaskRows = $derived(backgroundHoldTaskRows(isLive ? backgroundHold : null));
  let hasQueuedMessages = $derived(queuedMessages.length > 0);
  let noteFollowupLabel = $derived(getNoteFollowupLabel(session, messages, noteInfo));
  let assistantMarkdownContent = $derived(
    messages
      .filter((message) => message.role === 'assistant')
      .map((message) => message.content)
      .join('\n\n')
  );
  // Reuse group/item identities across rebuilds so the keyed each only
  // re-evaluates groups whose data actually changed — a rebuild happens twice
  // per second while a session streams.
  let previousGrouped: AcpTranscriptGroup[] = [];
  let grouped = $derived.by(() => {
    previousGrouped = stabilizeAcpTranscriptGroups(
      previousGrouped,
      buildAcpTranscriptGroups(
        messages,
        metadataRowsForWindow(acpMetadataMessages, messages, hasOlderMessages),
        displayRoots
      )
    );
    return previousGrouped;
  });
  /** Number-valued so per-group template expressions can depend on "am I the
   *  last group?" without re-running whenever the grouped array identity
   *  changes — deriveds cut propagation when the value is equal. */
  let lastGroupIndex = $derived(grouped.length - 1);
  /** Latest ACP plan, pinned above the transcript rather than rendered in it. */
  let plan = $derived.by(() => latestPlan(acpMetadataMessages));
  /** Pikchr sources of successful render_pikchr tool calls, shown inline as diagrams. */
  let pikchrToolSources = $derived(
    grouped.flatMap((group) =>
      group.type === 'tools'
        ? group.items
            .map((item) => item.pikchrRenderSource)
            .filter((source): source is string => source !== null)
        : []
    )
  );
  let sessionHasPikchr = $derived(
    pikchrToolSources.length > 0 ||
      extractMarkdownDiagramFences(assistantMarkdownContent).some(
        (diagram) => diagram.language === 'pikchr'
      )
  );
  let pikchrRenderer = $state<PikchrRenderer | null>(null);
  let pikchrRendererLoadKey = $derived(
    `${sessionId}\0${assistantMarkdownContent}\0${pikchrToolSources.join('\0')}`
  );
  let pikchrRendererLoadFailedKey = $state<string | null>(null);
  // See the loader effect below: the chat never waits blank on this, so the cap is loose.
  const SESSION_PIKCHR_LOAD_TIMEOUT_MS = 60_000;
  let pikchrRendererLoadFailed = $derived(pikchrRendererLoadFailedKey === pikchrRendererLoadKey);
  let mediaViewer = $state<MarkdownMedia | null>(null);
  let diagramViewerSvg = $state<string | null>(null);
  let metadataFollowupConfig = $derived.by(() =>
    latestAcpConfigDiscoveryFromMetadata(session?.provider, acpMetadataMessages)
  );
  let rediscoveredFollowupConfig = $derived(
    discoveredFollowupConfig?.providerId === session?.provider ? discoveredFollowupConfig : null
  );
  let usesModelSpecificFollowupConfig = $derived(
    !!modelSpecificFollowupModelValue &&
      modelSpecificFollowupModelValue === selectedFollowupModelValue &&
      modelSpecificFollowupSessionId === session?.id &&
      followupModelTouched
  );
  let activeModelSpecificFollowupConfig = $derived(
    usesModelSpecificFollowupConfig && modelSpecificFollowupConfig?.providerId === session?.provider
      ? modelSpecificFollowupConfig
      : null
  );
  let followupModelSelector = $derived(
    activeModelSpecificFollowupConfig?.model ??
      metadataFollowupConfig?.model ??
      rediscoveredFollowupConfig?.model ??
      null
  );
  let followupEffortSelector = $derived.by(() => {
    if (usesModelSpecificFollowupConfig) {
      return activeModelSpecificFollowupConfig?.effort ?? null;
    }
    return metadataFollowupConfig?.effort ?? rediscoveredFollowupConfig?.effort ?? null;
  });
  let followupProviderLabel = $derived(
    agentState.providers.find((provider) => provider.id === session?.provider)?.label ??
      session?.provider ??
      null
  );
  let followupAcpConfigSelection = $derived(
    buildAcpConfigSelection({
      model: {
        selector: followupModelSelector,
        valueId: selectedFollowupModelValue,
        explicit: shouldSendFollowupSelectorSelection(
          followupModelSelector,
          selectedFollowupModelValue,
          session?.acpConfigSelection?.model ?? null,
          followupModelTouched
        ),
      },
      effort: {
        selector: followupEffortSelector,
        valueId: selectedFollowupEffortValue,
        explicit: shouldSendFollowupSelectorSelection(
          followupEffortSelector,
          selectedFollowupEffortValue,
          usesModelSpecificFollowupConfig && !followupEffortTouched
            ? null
            : (session?.acpConfigSelection?.effort ?? null),
          followupEffortTouched
        ),
      },
    })
  );

  const SLIDE_DURATION = 150;

  $effect(() => {
    const rootCandidates: DisplayRootInput = [repoDir, session?.workingDir];
    const nextKey = displayRootKey(rootCandidates);
    if (nextKey === currentDisplayRootKey) return;

    currentDisplayRootKey = nextKey;
    displayRoots = normalizeDisplayRoots(rootCandidates);

    let stale = false;
    if (nextKey) {
      resolveDisplayRoots(rootCandidates).then((resolvedRoots) => {
        if (!stale && currentDisplayRootKey === nextKey) {
          displayRoots = resolvedRoots;
        }
      });
    }

    return () => {
      stale = true;
    };
  });

  /** Whether a mounting group slides in: off until the initial load has rendered
   *  and around merges that are not live updates. */
  const intro = createIntroGate();

  // Hashtag reference items
  let hashtagItems = $state<HashtagItem[]>([]);
  $effect(() => {
    if (providedHashtagItems) {
      hashtagItems = providedHashtagItems;
      return;
    }

    if (!branchId) {
      hashtagItems = [];
      return;
    }

    let stale = false;
    buildBranchHashtagItems(branchId, projectId ?? null, {
      repoSlug: repoLabel?.headRepo ?? repoLabel?.githubRepo,
      repoSubpath: repoLabel?.subpath,
    }).then((items) => {
      if (!stale) hashtagItems = items;
    });
    return () => {
      stale = true;
    };
  });

  // Image attachment state (available when project context is provided; branchId is optional)
  let canAttachImages = $derived(!!projectId);
  let replyImageIds = $state<string[]>([]);
  // Image ID → preview; null once a load failed, so it is not retried forever.
  let imagePreviews = $state<Map<string, AttachmentPreview | null>>(new Map());
  let attachError = $state<string | null>(null);

  // Drag-and-drop state
  let dragOver = $state(false);
  let modalElement: HTMLElement | null = $state(null);

  // Preview cache for message history and queued-message attachments.
  // Maps image ID → preview (null: failed). Loaded lazily when needed.
  let messageImageCache = $state<Map<string, AttachmentPreview | null>>(new Map());

  // Load previews for attached images and PDFs
  $effect(() => {
    for (const id of replyImageIds) {
      if (!imagePreviews.has(id)) {
        getAttachmentThumbnail(id)
          .then((preview) => {
            imagePreviews = new Map(imagePreviews);
            imagePreviews.set(id, preview);
          })
          .catch(() => {
            // Image may have been deleted
            imagePreviews = new Map(imagePreviews);
            imagePreviews.set(id, null);
          });
      }
    }
  });

  // Load image data for images referenced in message history
  $effect(() => {
    const allImageIds = new Set<string>();
    for (const msg of messages) {
      if (msg.imageIds) {
        for (const id of msg.imageIds) {
          allImageIds.add(id);
        }
      }
    }
    for (const msg of queuedMessages) {
      for (const id of msg.imageIds) {
        allImageIds.add(id);
      }
    }
    for (const id of allImageIds) {
      if (!messageImageCache.has(id)) {
        getAttachmentThumbnail(id)
          .then((preview) => {
            messageImageCache = new Map(messageImageCache);
            messageImageCache.set(id, preview);
          })
          .catch(() => {
            messageImageCache = new Map(messageImageCache);
            messageImageCache.set(id, null);
          });
      }
    }
  });

  // Picker and paste batches: the error is cleared once up front and reported
  // once at the end, so a later success cannot hide an earlier rejection.
  async function handleAttachFiles(files: File[]) {
    if (!projectId) return;
    attachError = null;
    // Attaching is a draft edit: the last failure line has been read.
    sendError = null;
    const rejections: AttachRejection[] = [];
    for (const file of files) {
      const reason = await addImageFile(file);
      if (reason !== null) rejections.push({ name: file.name, reason });
    }
    attachError = formatAttachErrors(rejections);
  }

  /** Stores one file, returning the rejection reason or null on success. */
  async function addImageFile(file: File): Promise<string | null> {
    if (!projectId) return null;
    try {
      const base64 = await fileToBase64(file);
      const image = await createImageFromData(
        branchId ?? null,
        projectId,
        file.name,
        file.type,
        base64,
        true
      );
      replyImageIds = [...replyImageIds, image.id];
      // Preview from the local bytes when they can stand in for the stored
      // file; otherwise (relabelled image, PDF) the effect fetches the
      // backend thumbnail.
      const preview = localPreview(image, file.type, base64);
      if (preview) {
        imagePreviews = new Map(imagePreviews);
        imagePreviews.set(image.id, preview);
      }
      return null;
    } catch (err) {
      console.error('Failed to attach image:', err);
      return attachErrorReason(err);
    }
  }

  function handleImagePaste(e: ClipboardEvent) {
    if (!active || !canAttachImages || isLive) return;
    const items = e.clipboardData?.items;
    if (!items) return;
    const files: File[] = [];
    for (const item of Array.from(items)) {
      if (isAttachableClipboardType(item.type)) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
    if (files.length > 0) void handleAttachFiles(files);
  }

  function removeReplyImage(imageId: string) {
    replyImageIds = replyImageIds.filter((id) => id !== imageId);
    imagePreviews = new Map(imagePreviews);
    imagePreviews.delete(imageId);
    attachError = null;
    // Removing an attachment is a draft edit too.
    sendError = null;
    deleteImage(imageId).catch((err) => {
      console.error('Failed to delete image:', err);
    });
  }

  // Drag-and-drop files (via Tauri native drag-drop events)
  async function handleFileDrop(paths: string[]) {
    if (!projectId) return;
    const imagePaths = paths.filter((p) => isImageAttachmentFile(p));
    const textPaths = paths.filter((p) => isMaybeTextFile(p));
    const bid = branchId ?? null;
    const pid = projectId;
    const newIds: string[] = [];
    // Surface rejection reasons (signature, size) as paste does, reporting
    // the whole batch once so a later success cannot hide an earlier one.
    attachError = null;
    sendError = null;
    const rejections: AttachRejection[] = [];
    for (const path of imagePaths) {
      try {
        const image = await createImage(bid, pid, path, true);
        newIds.push(image.id);
      } catch (e) {
        console.error('Failed to create image from dropped file:', e);
        rejections.push({ name: attachBasename(path), reason: attachErrorReason(e) });
      }
    }
    attachError = formatAttachErrors(rejections);
    if (newIds.length > 0) {
      replyImageIds = [...replyImageIds, ...newIds];
    }
    if (textPaths.length > 0 && inputEl) {
      insertFilePathsAtCursor(inputEl, textPaths);
    }
  }

  // Subscribe to the shared drag-drop service
  $effect(() => {
    const el = modalElement;
    if (!active || !el || !canAttachImages) return;
    const unsub = untrack(() =>
      subscribeDragDrop({
        element: el,
        blocking: true,
        onDragOver: (over) => {
          dragOver = over;
        },
        onDrop: (paths) => {
          handleFileDrop(paths);
        },
      })
    );
    return unsub;
  });

  // Search state
  let searchQuery = $state('');
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let matchElements: HTMLElement[] = [];

  // =========================================================================
  // Lifecycle
  // =========================================================================

  onDestroy(() => {
    closed = true;
    destroyed = true;
    // A restore still waiting on its frame has nothing to scroll once the pane
    // is gone; cancel it here rather than rely on the pool parking first.
    deferredRestore.take();
    stopPolling();
    unlistenStatus?.();
    unlistenBackgroundHold?.();
    if (checkNowTimer) clearTimeout(checkNowTimer);
    // Flush (not cancel) so a reply typed within the debounce window of
    // teardown still gets persisted.
    void replyDraftWriter.flush();
  });

  // This pane is mounted once and reused across opens (`sessionPanePool`
  // parks it between dialogs and the `active` prop toggles visibility), so the
  // session must be (re)loaded reactively rather than in onMount — otherwise it
  // would load once with whatever sessionId was set at mount time and never
  // refresh. Re-run whenever the modal is opened or the target session changes.
  $effect(() => {
    // Track active + sessionId only; loadSession()'s own state writes must not
    // retrigger this effect.
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id) {
      closed = true;
      stopPolling();
      return;
    }
    // Reset the teardown guard deterministically before loading, in the same
    // flush. `closed` is a plain (non-reactive) variable, so writing it here
    // does not retrigger this effect — which is exactly why relying on a
    // separately-declared reset effect raced ahead of this load.
    closed = false;
    stopPolling();
    const mode = untrack(() =>
      reopenMode({
        loadedSessionId: session?.id,
        sessionId: id,
        messageCount: messages.length,
        error,
      })
    );
    if (mode === 'revalidate') {
      // The transcript and composer are already built and attached, so focus
      // on this open's first frame rather than after the round trip the full
      // load waits for. A frame, not a microtask: `unpark` requested its
      // scroll restore's frame before `active` flipped, so this callback runs
      // after it, once the restore has already paid the frame's style and
      // layout — a focus from `tick()` landed between the mount flush and
      // that frame and forced a style pass of its own over the new dialog.
      requestAnimationFrame(() => focusComposerWhileOpen(id));
    }
    loadSession(mode).then(() => {
      if (closed || !active || sessionId !== id) return;
      if (session?.status === 'running') {
        startPolling();
      }
      // Focus input on open
      if (mode === 'full') tick().then(() => focusComposerWhileOpen(id));
    });
  });

  // The focus can wait out a full-screen dialog's enter animation, and the
  // pane may be parked or retargeted by then, so the open is re-checked when
  // the focus lands rather than when it was asked for. On a touch screen the
  // composer is left for the user to tap (see autofocusOnOpen).
  function focusComposerWhileOpen(id: string) {
    autofocusOnOpen(inputEl, (el) => {
      if (!closed && active && sessionId === id) el.focus();
    });
  }

  $effect(() => {
    onSessionChange?.(session);
  });

  $effect(() => {
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id) {
      loadedDraftSessionId = null;
      return;
    }

    const generation = ++draftLoadGeneration;
    const draftWhenLoadStarted = untrack(() => cloneComposerDraft(currentDraft()));
    suppressNextReplySave = false;
    loadedDraftSessionId = null;
    // Another session's draft is coming up: an Undo for a block removed from
    // the previous one would read as this one's.
    dismissQuoteUndo((sid) => sid === id);
    replyDraftLoad = loadReplyDraft(id, generation, draftWhenLoadStarted);
  });

  // While this pane has the session's composer on screen, a draft returned
  // to that session by a pane that no longer shows it belongs in this box
  // (see `putBackUnstartedDraft`); and this pane's own toast about a draft it
  // put back while parked has been answered by opening the chat.
  $effect(() => {
    const id = sessionId;
    if (!active || !id) return;
    if (unstartedDraftToastShown?.sessionId === id) {
      toast.dismiss(unstartedDraftToastShown.toastId);
      unstartedDraftToastShown = null;
    }
    return offerUnstartedDraftBox(id, putBackUnstartedDraft);
  });

  $effect(() => {
    const isActive = active;
    const id = sessionId;
    // Reads every field of the draft, so any edit — the main input, a gap,
    // a quote's reply — schedules a save.
    const content = composerDraftContent(currentDraft());
    if (!isActive || !id || loadedDraftSessionId !== id) return;
    if (suppressNextReplySave) {
      suppressNextReplySave = false;
      return;
    }
    replyDraftWriter.scheduleSave(replyDraftKey(id), content);
  });

  // While this pane shows a session's draft it takes the quotes a note adds
  // to it (see replyDrafts.ts). Quotes still waiting on the draft's load when
  // the pane moves on go to the stored draft instead of being dropped.
  $effect(() => {
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id) return;
    const unregister = registerReplyComposer(id, {
      addQuote: (quote, quoteId) =>
        receiveDraftEdit(id, (draft) => appendDraftQuote(draft, quote, quoteId)),
      setQuoteReply: (quoteId, reply) =>
        receiveDraftEdit(id, (draft) => setDraftQuoteReply(draft, quoteId, reply)),
      restoreQuote: (removed) => receiveDraftEdit(id, (draft) => restoreDraftQuote(draft, removed)),
      focusQuoteReply: (quoteId) => whenDraftLoaded(id, () => focusDraftQuoteReply(quoteId)),
    });
    return () => {
      unregister();
      const pending = pendingDraftEdits;
      if (pending?.sessionId !== id) return;
      pendingDraftEdits = null;
      amendStoredReplyDraft(id, (draft) =>
        pending.edits.reduce((next, { edit }) => edit(next), draft)
      ).catch((err) => {
        console.error('[SessionChatPane] Failed to save quoted reply:', err);
        toast.error('Could not save the quote in your reply draft');
      });
    };
  });

  $effect(() => {
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id) return;

    const unlisten = listenToEvent<SessionStatusPayload>('session-status-changed', (payload) => {
      if (payload.sessionId !== id) return;
      statusEventVersion += 1;
      if (session?.id === id) {
        session = {
          ...session,
          status: payload.status,
          errorMessage: payload.errorMessage ?? session.errorMessage,
          completionReason: payload.completionReason ?? session.completionReason,
          updatedAt: Date.now(),
        };
        // Applying the payload makes the status current even mid-load, so a
        // hold reported after it is judged against this rather than treated as
        // unknown. (A payload for a session that isn't loaded is dropped, as
        // it always was, and must not mark anything current.)
        statusLoadedForSessionId = id;
      }
      refreshQueuedMessages();
      if (payload.status === 'running') {
        startPolling();
      } else {
        // A terminal status ends any wait: the agent has been torn down.
        clearBackgroundHold();
        liveDelivery = LIVE_DELIVERY_UNKNOWN;
        finishPolling();
      }
    });
    unlistenStatus = unlisten;

    return () => {
      unlistenStatus?.();
      unlistenStatus = null;
    };
  });

  // A `disconnected` session ends when reconcile settles it, and only that
  // status event tells this pane. Should it be missed (a web socket that
  // reconnects across it, say), the pane would go on showing "Running on Blox"
  // for a turn that finished. So while the session is disconnected, re-fetch
  // it on a slow timer. Each fetch also brings in the transcript reconcile
  // backfills while catching up.
  $effect(() => {
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id || !isDisconnected) return;

    const timer = setInterval(() => refetchReconciled(id), DISCONNECTED_REFETCH_MS);
    return () => clearInterval(timer);
  });

  // And re-fetch it at once when reconcile goes idle for it: it has stopped
  // working on the row, usually because it settled it, or a Stop's backfill of
  // a Blox turn is done. That backfill goes on after the Stop has ended the
  // row, so this listens whatever the session's status.
  $effect(() => {
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id) return;

    return listenToEvent<SessionReconcileEvent>('session-reconcile-changed', (event) => {
      if (event.sessionId === id && event.phase === 'idle') refetchReconciled(id);
    });
  });

  /** Re-fetch session `id`, which reconcile may have moved on or backfilled. */
  function refetchReconciled(id: string) {
    if (closed || sessionId !== id) return;
    void poll().then(() => {
      // A live reattach took the session back to running.
      if (!closed && sessionId === id && session?.status === 'running') startPolling();
    });
  }

  // The session stays `running` while its agent is held open for background
  // work, so the wait arrives on its own event rather than as a status change.
  $effect(() => {
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id) return;

    backgroundHold = null;
    liveDelivery = LIVE_DELIVERY_UNKNOWN;
    stoppingTaskIds = new Set();
    taskStopNotices = new Map();
    const unlisten = listenToEvent<SessionBackgroundHoldPayload>(
      'session-background-hold',
      (payload) => {
        if (payload.sessionId !== id) return;
        backgroundHoldReportVersion += 1;
        applyBackgroundHold(id, payload);
      },
      {
        // The event above is emitted on change, so a pane mounting mid-hold
        // has already missed every report for it — with one long task, that is
        // the entire wait. Ask for the current hold, from inside the hook that
        // says this listener is live: the backend records the retained
        // snapshot before it emits, so with the listener up first, every
        // report is either contained in the answer below or delivered as an
        // event. Nothing can fall between. Requesting beside the
        // `listenToEvent` call instead would leave the registration window
        // (a `listen()` roundtrip on Tauri, a connecting socket on web) as a
        // hole this guard cannot see.
        //
        // The overlap is arbitrated by the version: a live report that lands
        // while the request is in flight wins, being newer by definition. It
        // is captured per fire rather than once — the hook fires again on
        // every web reconnect, and a version captured at effect time would be
        // permanently behind by then, discarding every later snapshot.
        onEstablished: () => {
          const versionAtEstablished = backgroundHoldReportVersion;
          getSessionBackgroundHold(id)
            .then((snapshot) => {
              if (closed || sessionId !== id) return;
              if (backgroundHoldReportVersion !== versionAtEstablished) return;
              applyBackgroundHold(id, snapshot);
            })
            .catch(() => {
              // Best-effort catch-up: the next report still paints the wait.
            });
        },
      }
    );
    unlistenBackgroundHold = unlisten;

    return () => {
      unlistenBackgroundHold?.();
      unlistenBackgroundHold = null;
    };
  });

  /**
   * Adopt a hold reported for `id` — from the event or the mount-time snapshot
   * — and reconcile the per-task stops in flight against it. A stop stays
   * marked until its row leaves the reported set, which is what proves it took.
   *
   * The report is judged against the session's status only when that status is
   * known to be this open's: a pane reopened on a session it already showed
   * still carries the status from last time until its load resolves, and
   * mistaking that for the current one would discard the very hold the
   * mounting pane just asked for.
   */
  function applyBackgroundHold(id: string, report: SessionBackgroundHold) {
    backgroundHold = nextBackgroundHold(
      report,
      knownSessionStatus(id, session, statusLoadedForSessionId)
    );
    const pruned = pruneStoppingTaskIds(stoppingTaskIds, backgroundHold);
    // Pruning only ever removes, so a size match means nothing changed.
    if (pruned.size !== stoppingTaskIds.size) stoppingTaskIds = pruned;
    const prunedNotices = pruneTaskStopNotices(taskStopNotices, backgroundHold);
    if (prunedNotices.size !== taskStopNotices.size) taskStopNotices = prunedNotices;
  }

  /** Withdraw the wait: no hold, and no per-task stop left to be in flight. */
  function clearBackgroundHold() {
    backgroundHold = null;
    if (stoppingTaskIds.size > 0) stoppingTaskIds = new Set();
    if (taskStopNotices.size > 0) taskStopNotices = new Map();
  }

  function isComposerFocused(): boolean {
    return document.activeElement === inputEl;
  }

  // =========================================================================
  // Data loading
  // =========================================================================

  /**
   * Bring the pane up to date for `sessionId`. `revalidate` (a reopen of the
   * session this pane already shows, see `reopenMode`) keeps the rendered
   * transcript and fetches one delta; `full` is the initial load.
   */
  async function loadSession(mode: ReopenMode = 'full') {
    if (closed) return;
    if (mode === 'revalidate') {
      if ((await revalidateSession()) === 'settled' || closed) return;
      // The delta could not stand in for a load (the session is gone, or the
      // request failed): fall through to the full load, which shows the same
      // error state a fresh open would and never starts polling a dead session.
    }
    // Clear prior session state when loading a *different* session so a
    // bail-out or in-flight load can never paint the previously-viewed
    // session — show a clean loading state for the requested id instead. When
    // reopening the same session, keep the existing transcript so it stays
    // visible (and correct) rather than flashing a loading state. Read the
    // currently-loaded id via untrack so this synchronous prologue never
    // subscribes the calling load effect to `session` — which it mutates
    // below, otherwise re-triggering the effect in a tight infinite loop.
    if (untrack(() => session?.id) !== sessionId) {
      session = null;
      messages = [];
      hasOlderMessages = false;
      acpMetadataMessages = [];
      queuedMessages = [];
      // A failure line is about the session that was shown when it happened.
      sendError = null;
    }
    loading = true;
    error = null;
    // Whatever `session.status` says right now predates this load: on a reopen
    // it is the status from the last time the pane showed this session. Nobody
    // may judge a hold report against it until the load below replaces it.
    statusLoadedForSessionId = null;
    const statusVersionBeforeFetch = statusEventVersion;
    const id = sessionId;
    try {
      // Only the newest page of the transcript; older pages arrive through
      // `loadOlderMessages` as the user scrolls up.
      const [s, msgsResult, acpMsgs, queued] = await Promise.all([
        getSession(sessionId),
        getLatestSessionMessages(sessionId, pageRequestLimit(TRANSCRIPT_PAGE_ROWS)),
        getSessionAcpMetadataMessages(sessionId),
        listQueuedSessionMessages(sessionId),
      ]);
      if (closed) return;
      if (!s) {
        // On a reopen of a deleted session the stale `session` (kept above so
        // the transcript stays put) still carries its last status, possibly
        // `running`; the load effect would read that and start polling a
        // session nothing will ever update. Drop it so this ends in the same
        // state a fresh open of a deleted session does.
        session = null;
        error = 'Session not found';
        return;
      }
      // A status event applied to this session mid-fetch is newer than what
      // these queries read, so adopting `s` would revert the status *and* mark
      // the reverted value current for this open — the marker the hold's
      // freshness guard trusts. Leave both to the event, which set them itself.
      // The rest of the row is skipped with them, the same trade the poll path
      // makes; a session left running is re-fetched by the poll it just started.
      if (!statusEventSupersededLoad(statusVersionBeforeFetch, statusEventVersion, session, s.id)) {
        session = s;
        statusLoadedForSessionId = s.id;
      }
      // A hold reported while the status was unknown was kept on trust; now
      // that the real status is in, re-judge it — a snapshot answered for a
      // session that had already finished would otherwise sit unrendered
      // behind `isLive` until some later flip back to `running` surfaced it.
      if (backgroundHold) applyBackgroundHold(s.id, backgroundHold);
      const page = transcriptPageFromRows(msgsResult.data, TRANSCRIPT_PAGE_ROWS);
      messages = page.messages;
      hasOlderMessages = page.hasMore;
      queuedMessages = queued;
      if (msgsResult.revalidating) {
        msgsResult.revalidating
          .then((fresh) => {
            if (closed || sessionId !== id) return;
            // The user may have paged back past the cached window by now;
            // those pages are kept below the fresh copy of the window.
            const next = applyLatestWindow(
              { messages, hasMore: hasOlderMessages },
              transcriptPageFromRows(fresh, TRANSCRIPT_PAGE_ROWS)
            );
            messages = next.messages;
            hasOlderMessages = next.hasMore;
            scrollToBottomIfNear(true);
          })
          .catch(() => {});
      }
      acpMetadataMessages = acpMsgs;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
      // Wait for Svelte to render the messages (they only mount once loading
      // is false) before enabling intro transitions, so existing messages
      // don't all slide in at once.
      await tick();
      intro.enable();
      scrollToBottom();
      // A first page shorter than the viewport leaves nothing to scroll, so
      // the scroll handler alone could never ask for the next one.
      await tick();
      loadOlderMessagesIfUnscrollable();
    }
  }

  /**
   * Catch a parked pane up without touching `loading`, whose branch would swap
   * the transcript for a spinner and rebuild every group. One `poll_session`
   * delta returns the status, the queue, the rows written while the pane was
   * parked and any that changed, and the metadata past the cursor. Groups the
   * delta appends must not slide in — this is a reopen, not a live update —
   * so the intro transition is suppressed around the merge as the load-older
   * path does. A long park on a busy session can return a delta far larger
   * than a page; the window is bounded afterwards so the `{#each}` below stays
   * about a page and `hasOlderMessages` pages the rest back in. Resolves
   * `'full'` when the delta could not bring the pane up to date (see
   * `reopenAfterRevalidate`); the caller runs the full load then.
   */
  async function revalidateSession(): Promise<'settled' | 'full'> {
    // Whatever `session.status` says now is from the last time this pane was
    // shown; a hold must not be judged against it until the delta lands.
    statusLoadedForSessionId = null;
    const id = sessionId;
    const rowsBeforeDelta = messages.length;
    const resumeIntros = intro.suspend();
    try {
      const outcome = await poll();
      if (closed || sessionId !== id) return 'settled';
      // The restore `unpark` deferred has normally run by now (the delta is a
      // round trip, the frame is next in line), but a hidden window never
      // fires frames: apply it before anything below measures the box.
      deferredRestore.flush();
      const next = reopenAfterRevalidate(outcome);
      if (next === 'full') return next;
      if (session) statusLoadedForSessionId = session.id;
      if (backgroundHold) applyBackgroundHold(id, backgroundHold);
      const bounded = trimLatestWindow(
        { messages, hasMore: hasOlderMessages },
        TRANSCRIPT_PAGE_ROWS,
        rowsBeforeDelta
      );
      if (bounded.messages !== messages) {
        // `unpark` already restored the scroll position against the untrimmed
        // rows, and WebKit has no scroll anchoring: dropping rows above the
        // viewport would shift it onto other rows and hand `handleScroll` a
        // spurious upward scroll. Hold the first visible group in place; when
        // the trim removed it follow the transcript to the bottom rather than
        // land at the top and page the trimmed rows back. In practice the
        // bottom is the path taken: `trimLatestWindow` only trims when the
        // delta alone exceeded a page and then keeps only the last page, so
        // every pre-delta row — including whichever group `unpark` put in
        // view — is gone. The hold branch is kept as the safety net for a
        // trim rule that ever retains pre-delta rows, not as the common case.
        const el = messagesEl;
        const anchor = el && userScrolledUp ? captureScrollAnchor(el) : null;
        messages = bounded.messages;
        hasOlderMessages = bounded.hasMore;
        await tick();
        if (el) {
          if (!anchor || !restoreScrollAnchor(el, anchor)) {
            el.scrollTop = el.scrollHeight;
            userScrolledUp = false;
          }
          noteScrollPosition(el);
        }
      }
      // Otherwise `unpark` restored the scroll position and `poll` follows the
      // bottom when its merge appends, so nothing here forces layout on the open.
      return next;
    } finally {
      await tick();
      resumeIntros();
    }
  }

  /** Incremental poll — one `pollSession` request per tick returns the status,
   *  the follow-up queue, the last message (may have grown) + any new ones, and
   *  the metadata rows past the cursor, all from one backend snapshot.
   *
   *  State assignments are skipped whenever a tick brings no actual change:
   *  reassigning the same data under a fresh identity would invalidate the
   *  grouped transcript and re-render the entire message list twice a second.
   */
  async function poll(): Promise<PollOutcome> {
    if (!session || closed || pollInFlight) return 'skipped';
    pollInFlight = true;
    try {
      const statusVersionBeforeFetch = statusEventVersion;
      // An empty transcript polls from 0, which returns every visible row, so
      // there is nothing older to page in: `hasOlderMessages` stays false.
      const request = sessionPollRequest(messages, acpMetadataMessages);
      const result = await pollSession(sessionId, request);
      if (closed) return 'skipped';
      const s = result.session;
      const statusEventDuringFetch = statusEventVersion !== statusVersionBeforeFetch;
      if (s && !statusEventDuringFetch && JSON.stringify(s) !== JSON.stringify(session)) {
        session = s;
      }
      if (JSON.stringify(result.queuedMessages) !== JSON.stringify(queuedMessages)) {
        queuedMessages = result.queuedMessages;
      }
      refreshLiveDelivery(s?.status ?? session?.status);

      applyAcpMetadataUpdates(result.acpMetadataMessages);
      const merged = mergeSinceMessages(messages, result.messages, request.sinceId);
      if (merged) {
        messages = merged.messages;
        if (merged.appended) scrollToBottomIfNear(true);
      }

      // Stop polling when the session is done or gone (`pollShouldStop`). The
      // snapshot that reports a terminal status already holds the rows written
      // before the flip, so the last text is in `merged` above. A missing
      // session ends the loop whatever the stale local status says: nothing
      // will ever flip it back.
      if (pollShouldStop(s, statusEventDuringFetch, session?.status)) {
        clearPollTimer();
      }
      return s ? 'applied' : 'missing';
    } catch {
      // Polling errors are expected during shutdown — silently ignore
      return 'failed';
    } finally {
      pollInFlight = false;
      if (finalPollPending) {
        finalPollPending = false;
        void poll();
      }
    }
  }

  /**
   * Merge incremental metadata rows into the cached array. Row identity is
   * preserved when a re-fetched unsettled row comes back unchanged, and array
   * identity is preserved when no row changed at all, so unchanged transcript
   * groups keep their identity downstream. A row that settled since the last
   * tick arrives with its large bodies deferred; the inline bodies already held
   * are kept where they match (`retainInlineToolBodies`), so an expanded card
   * does not flip to loading and refetch the bytes it was showing.
   */
  function applyAcpMetadataUpdates(updates: SessionMessage[]) {
    if (updates.length === 0) return;
    const byId = new Map(acpMetadataMessages.map((message) => [message.id, message]));
    let changed = false;
    for (const fetched of updates) {
      const current = byId.get(fetched.id);
      const update = current ? retainInlineToolBodies(current, fetched) : fetched;
      if (current && JSON.stringify(current) === JSON.stringify(update)) continue;
      byId.set(update.id, update);
      changed = true;
    }
    if (!changed) return;
    acpMetadataMessages = [...byId.values()].sort((a, b) => a.id - b.id);
  }

  /**
   * Learn whether the running session can take a message now. One cheap
   * registry lookup per poll tick while the answer is a retryable no; nothing
   * once it is settled (see `liveDelivery`). A status other than `running`
   * forgets the answer at once, so the next run asks afresh.
   */
  function refreshLiveDelivery(status: SessionStatus | undefined) {
    if (status !== 'running') {
      liveDelivery = LIVE_DELIVERY_UNKNOWN;
      return;
    }
    if (!shouldAskLiveDelivery(status, liveDelivery) || liveDeliveryInFlight || !sessionId) {
      return;
    }
    const id = sessionId;
    liveDeliveryInFlight = true;
    getSessionLiveDelivery(id)
      .then((capability) => {
        if (closed || sessionId !== id || session?.status !== 'running') return;
        liveDelivery = liveDeliveryKnowledge(capability);
      })
      .catch(() => {
        // Best-effort: the next tick asks again.
      })
      .finally(() => {
        liveDeliveryInFlight = false;
      });
  }

  async function refreshQueuedMessages() {
    if (!sessionId || closed) return;
    try {
      const queued = await listQueuedSessionMessages(sessionId);
      if (JSON.stringify(queued) !== JSON.stringify(queuedMessages)) {
        queuedMessages = queued;
      }
    } catch (e) {
      console.error('Failed to refresh queued messages:', e);
    }
  }

  function startPolling() {
    if (pollTimer) return;
    pollTimer = setInterval(poll, 500);
  }

  /** Abandon polling: the pane is closing or switching session. */
  function stopPolling() {
    clearPollTimer();
    pollInFlight = false;
    finalPollPending = false;
  }

  /**
   * Stop the interval and poll once more. Teardown writes the final text of an
   * open assistant row just before the status flips, which the last interval
   * tick may have missed. A tick still in flight read the same cursor this
   * poll would, and the two merges would duplicate the rows both fetched, so
   * the final poll is deferred until that tick settles instead.
   *
   * `pollInFlight` is left alone: it belongs to the tick's `finally`. Resetting
   * it here would reopen the guard while that tick is still awaiting, and a
   * `startPolling()` in the gap (a queue-drain running event right after the
   * terminal status, or a send) could run a tick concurrently against the
   * same cursor.
   */
  function finishPolling() {
    clearPollTimer();
    if (pollInFlight) {
      finalPollPending = true;
    } else {
      void poll();
    }
  }

  function clearPollTimer() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  // =========================================================================
  // Message sending & queue
  // =========================================================================

  let sending = $state(false);
  let noteFollowupSending = $state(false);
  /**
   * Why the last composer action failed (a refused or failed send, a queue
   * change or cancel that failed). Shown on a line inside the composer, not in
   * `error`: that state replaces the whole transcript with a centered card,
   * which hides the very conversation the user is trying to continue, and
   * (see `reopenMode`) would turn the next reopen into a full reload.
   * Cleared when the next action starts, when the draft is edited (text,
   * attachments), and when the pane moves to another session. Rendered by
   * `ChatComposer`, or by the pipeline stop area that stands in for it.
   */
  let sendError = $state<string | null>(null);

  /**
   * A composer action is starting: drop the previous action's failure line,
   * and a stale load error with it — the action's outcome is what the pane
   * should show now, and the poll a successful one starts refreshes the
   * transcript anyway.
   */
  function beginComposerAction() {
    sendError = null;
    error = null;
  }

  /**
   * Why the composer has no session row to submit to (loading, or the load
   * settled without one), or `null`. The submit control is disabled while
   * set, with a title saying which.
   */
  let composerBlock = $derived(
    composerSessionBlock({
      loading,
      hasSession: session !== null,
      disconnected: isDisconnected,
    })
  );

  async function handleSend() {
    // Without a row there is nothing to send to; the control is disabled, and
    // Cmd+Enter keeps the draft rather than taking it out of the box only to
    // put it back.
    if (composerBlock) return;
    // A send still in flight (a live send can take a few seconds: steer
    // acknowledgement, interrupt-and-reprompt, or a resume) owns the composer
    // until it settles; a second Cmd+Enter meanwhile must not clear the draft
    // typed since, so the guard comes before anything is taken out of the box.
    if (sending) return;
    // Kept whole so a failed send puts the quote blocks back, not just text.
    const draft = cloneComposerDraft(currentDraft());
    if (isComposerDraftEmpty(draft)) return;
    const text = serializeComposerDraft(draft);
    if (!text) return;
    void replyDraftWriter.clearNow(replyDraftKey(sessionId));
    setDraft(emptyComposerDraft());
    const imageIdsToSend = replyImageIds.length > 0 ? [...replyImageIds] : undefined;
    replyImageIds = [];
    imagePreviews = new Map();
    attachError = null;

    if (sendControl.action === 'send-now') {
      if (await sendMessageNow(text, imageIdsToSend, draft)) draftSent();
      return;
    }

    if (isLive) {
      beginComposerAction();
      try {
        await queueSessionMessage(sessionId, text, imageIdsToSend, branchId ?? null);
        draftSent();
        await refreshQueuedMessages();
      } catch (e) {
        sendError = `Failed to queue: ${e instanceof Error ? e.message : String(e)}`;
        setDraft(draft);
        replyImageIds = imageIdsToSend ?? [];
      }
      return;
    }

    // Session is idle — send immediately. `sendMessage` reports the failure
    // itself; the draft goes back in the box so it can be retried.
    if (await sendMessage(text, imageIdsToSend)) {
      draftSent();
    } else {
      setDraft(draft);
      replyImageIds = imageIdsToSend ?? [];
    }
  }

  /**
   * Deliver a message to the running session now — the composer's "Send"
   * mode. The backend steers it into the turn in flight, or interrupts the
   * turn and re-prompts when the agent cannot steer. It never resumes: a
   * session that stopped between the last poll and the click comes back
   * `not_running`, and the message goes through `sendMessage` instead — the
   * idle path, which applies this pane's branch override and follow-up
   * config (the note chat pane's branch would otherwise be lost). Anything
   * not delivered (`blocked`, `rejected_queued`, `not_resumable`, or a kind
   * this pane does not know) is reported and the draft restored — never
   * re-queued behind the user's back, since a blocked steer may already have
   * landed. Resolves whether the message was delivered.
   */
  async function sendMessageNow(
    text: string,
    imageIds: string[] | undefined,
    draft: ComposerDraft
  ): Promise<boolean> {
    if (!session || sending) return false;
    const id = session.id;
    sending = true;
    beginComposerAction();
    try {
      const result = await sendSessionMessageNow(id, text, imageIds);
      if (session?.id !== id) return true;
      switch (result.delivery) {
        case 'injected':
        case 'interrupted_and_reprompted':
        case 'started_new_turn':
          // The user row is persisted by the delivery itself; the poll picks
          // it up.
          startPolling();
          scrollToBottom();
          return true;
        case 'resumed':
          // The command does not resume (see `not_running`); the kind belongs
          // to the shared MCP spelling. Should it ever arrive, the row is
          // running again on a fresh connection.
          session = { ...session, status: 'running' };
          clearBackgroundHold();
          liveDelivery = LIVE_DELIVERY_UNKNOWN;
          startPolling();
          scrollToBottom();
          return true;
        case 'not_running':
          // Stopped since the last poll: resume it the way the idle composer
          // does, with the branch override and follow-up config. `sending` is
          // already held here, so the lock is passed through. A resume that
          // fails (worktree gone, or the queue drain re-ran the session first
          // so the row is already running) has set `sendError`; fall through
          // to the draft restore below without overwriting it.
          liveDelivery = LIVE_DELIVERY_UNKNOWN;
          if (await sendMessage(text, imageIds, true)) return true;
          break;
        case 'rejected_queued':
          sendError = 'The session has not started yet; queue the message instead.';
          break;
        case 'blocked':
          // The connection may have gone under the request: ask again before
          // offering "Send" next time.
          liveDelivery = LIVE_DELIVERY_UNKNOWN;
          sendError = `Not delivered: ${result.reason ?? result.delivery}`;
          break;
        case 'not_resumable':
          // Defensive: the command refuses a stopped session before the
          // resume gate (`not_running`), so this kind only reaches the MCP
          // tool. Kept because the shared union still spells it.
          sendError = `Not delivered: ${result.reason ?? result.delivery}`;
          break;
        default: {
          // A delivery kind the backend added that this hand-maintained union
          // does not know: say so rather than restoring the draft in silence.
          const unknownKind: never = result.delivery;
          sendError = `Not delivered: ${String(unknownKind)}`;
          break;
        }
      }
      setDraft(draft);
      replyImageIds = imageIds ?? [];
      return false;
    } catch (e) {
      sendError = `Failed to send: ${e instanceof Error ? e.message : String(e)}`;
      setDraft(draft);
      replyImageIds = imageIds ?? [];
      return false;
    } finally {
      sending = false;
    }
  }

  /**
   * Actually send a message to the backend and start the agent. Resolves
   * `true` once the backend accepted the message (even if the pane has moved
   * to another session since), `false` when nothing was sent, and the caller
   * decides whether to restore its draft. `sendError` says why on `false` in
   * every case but two: when nothing could be targeted (defensive — the
   * composer is disabled until the row has loaded, and a targeted send checks
   * its target before calling), and when another send already holds
   * `sending`, since the spinner already explains the wait.
   */
  async function sendMessage(
    text: string,
    imageIds?: string[],
    sendingLocked = false,
    target: SendMessageTarget | null = null
  ): Promise<boolean> {
    const targetSessionId = target?.sessionId ?? session?.id;
    const targetBranchId = target ? target.branchId : (branchId ?? null);
    if (!targetSessionId || session?.id !== targetSessionId) {
      // The row is not loaded, or the pane moved on before a targeted send
      // ran. Nothing is said: a message here would be about a session this
      // pane no longer shows.
      return false;
    }
    if (!sendingLocked && sending) return false;
    if (!sendingLocked) sending = true;
    beginComposerAction();
    const acpConfigSelection: AcpConfigSelection | undefined =
      followupAcpConfigSelection ?? undefined;
    try {
      const outcome = await resumeSession(
        targetSessionId,
        text,
        imageIds,
        targetBranchId,
        acpConfigSelection
      );
      if (session?.id !== targetSessionId) return true;
      if (outcome.outcome === 'queuedForTransfer') {
        // Held by the backend until the project's transfer completes; the
        // session is still idle, so nothing starts polling. The queue shows
        // the message, labelled as waiting for the transfer.
        await refreshQueuedMessages();
        return true;
      }
      // Backend sets status to running and emits an event.
      // Force an immediate poll to pick up the new user message + status.
      session = {
        ...session,
        status: 'running',
        acpConfigSelection: acpConfigSelection ?? session.acpConfigSelection ?? null,
      };
      // A new turn is starting: any hold reported belonged to the previous
      // one, so drop it rather than letting this local flip back to `running`
      // re-render the old wait in place of "Thinking…". Likewise whatever was
      // known about live delivery was about the previous run.
      clearBackgroundHold();
      liveDelivery = LIVE_DELIVERY_UNKNOWN;
      modelSpecificFollowupConfig = null;
      modelSpecificFollowupModelValue = null;
      modelSpecificFollowupSessionId = null;
      observedDisconnect = null;
      startPolling();
      scrollToBottom();
      return true;
    } catch (e) {
      sendError = `Failed to send: ${e instanceof Error ? e.message : String(e)}`;
      return false;
    } finally {
      if (!sendingLocked) sending = false;
    }
  }

  /**
   * Start a new session from the draft instead of sending it: the composer's
   * "New note" / "New commit". The draft leaves the box and the store first,
   * as in `handleSend`, because the host closes the dialog as it starts the
   * session and this pane is parked with whatever the box still holds. A
   * start that failed (the host reports it, or throws) puts the draft back:
   * see `putBackUnstartedDraft`.
   */
  async function handleStartNewSession(kind: ComposerNewSessionKind) {
    const start = onStartNewSession;
    if (!start || newSessionDisabled) return;
    // Same gates as `handleSend`: a row to act on, no send in flight, text.
    if (composerBlock || sending) return;
    const draft = cloneComposerDraft(currentDraft());
    if (isComposerDraftEmpty(draft)) return;
    const text = serializeComposerDraft(draft);
    if (!text) return;
    const id = sessionId;
    void replyDraftWriter.clearNow(replyDraftKey(id));
    setDraft(emptyComposerDraft());
    const imageIds = [...replyImageIds];
    replyImageIds = [];
    imagePreviews = new Map();
    attachError = null;

    sending = true;
    beginComposerAction();
    let started = false;
    let reason: string | null = null;
    try {
      started = await start(kind, { text, imageIds });
    } catch (e) {
      reason = e instanceof Error ? e.message : String(e);
    } finally {
      sending = false;
    }
    if (started) return;
    await putBackUnstartedDraft(id, text, imageIds, reason);
  }

  /**
   * Put back a draft whose new session did not start, and say so.
   *
   * The start can take a while and the dialog is closed while it runs, so
   * the user may have reopened the note and typed something else (the box
   * stays editable while a send is in flight). The returned message never
   * replaces that text: it goes back above it (`restoreUnstartedDraft`), in
   * whichever of the two places holds the draft right now.
   *
   * - A pane on screen with its draft loaded holds the newest text in its
   *   box, so the join happens there, in one synchronous step, and is saved.
   * - Otherwise the store does: a parked pane's autosave does not run and
   *   the next open reloads the box from the store. Pending writes are
   *   flushed, the stored text read and the joined text written back. The
   *   write is queued in the same step as the last "still not on screen"
   *   check, so a reopen after it loads the joined text. The parked box is
   *   given the same text, so it agrees with the store even if that reload
   *   fails.
   * - A pane reopened in between is still loading its draft and does not yet
   *   know what its box should hold; the load is waited out first.
   * - When this pane is not showing the session but another pane is (this
   *   one was a transient pane, or was evicted from the pool, and the chat
   *   was opened again), that pane's box holds the newest text, and its next
   *   keystroke would save over anything written to the store behind its
   *   back. The draft is handed to it (`offerUnstartedDraftBox`).
   *
   * The failure is reported on the composer's own line, which is still
   * there when the note's chat is opened again, and by a toast when the pane
   * is not on screen to show it. A pane that is gone (unmounted, or showing
   * another session by now) has no box to point at: its toast says the
   * message was saved as the draft, and offers the message to copy.
   */
  async function putBackUnstartedDraft(
    id: string,
    text: string,
    imageIds: string[],
    reason: string | null
  ) {
    const draftKey = replyDraftKey(id);
    const onScreen = () => !destroyed && active && sessionId === id;
    const boxElsewhere = () =>
      onScreen() ? null : otherUnstartedDraftBox(id, putBackUnstartedDraft);
    for (;;) {
      const elsewhere = boxElsewhere();
      if (elsewhere) return elsewhere(id, text, imageIds, reason);
      // Bounded so a load that somehow never opens the gate cannot spin here
      // on microtasks; the box is then used as it stands.
      for (let waits = 0; waits < 8 && onScreen() && loadedDraftSessionId !== id; waits++) {
        // The load effect for this open may not have run yet.
        await tick();
        await replyDraftLoad;
      }
      if (onScreen()) {
        // The message goes above the main input; quote blocks added since
        // stay where they are, ahead of it.
        const restored = restoreUnstartedDraft(text, inputText);
        inputText = restored.text;
        replyImageIds = restoreUnstartedAttachments(imageIds, replyImageIds);
        void replyDraftWriter.saveNow(draftKey, composerDraftContent(currentDraft()));
        sendError = unstartedDraftMessage({ reason, keptNewerText: restored.keptNewerText });
        return;
      }

      // This pane's own box, parked: what its writer last sent to the store.
      const parkedBox = !destroyed && sessionId === id;
      // Every open starts a draft load, so this also tells an open that came
      // and went again while the store was being read.
      const loadsBeforeRead = draftLoadGeneration;
      let stored: ComposerDraft;
      try {
        await replyDraftWriter.flush();
        stored = await loadStoredReplyDraft(id);
      } catch (err) {
        console.error('[SessionChatPane] Failed to read the reply draft to restore into:', err);
        stored = parkedBox ? cloneComposerDraft(currentDraft()) : emptyComposerDraft();
      }
      // Reopened while the store was read, here or in another pane: a box is
      // the place after all. Or, if this pane was closed again too, the store
      // may hold newer text now.
      if (onScreen() || draftLoadGeneration !== loadsBeforeRead || boxElsewhere()) continue;
      const restored = restoreUnstartedDraft(text, stored.text);
      const restoredDraft: ComposerDraft = { quotes: stored.quotes, text: restored.text };
      void replyDraftWriter.saveNow(draftKey, composerDraftContent(restoredDraft));
      const boxGone = destroyed || sessionId !== id;
      if (!boxGone) {
        // Kept with the parked pane for its next open. That open reloads the
        // box from the store; until it has, and if it cannot, the box holds
        // the same draft (a parked pane's autosave does not run, so this
        // writes nothing). Attachments are not stored.
        setDraft(cloneComposerDraft(restoredDraft));
        replyImageIds = restoreUnstartedAttachments(imageIds, replyImageIds);
        sendError = unstartedDraftMessage({ reason, keptNewerText: restored.keptNewerText });
      }
      // A host that resolved `false` has reported why itself (the launch
      // helper's error toast); a throw is reported only here. Either stays
      // until dismissed or, for a parked pane, until its chat is opened
      // again: it says where the message went.
      const { title, description } = unstartedDraftToast({
        reason,
        keptNewerText: restored.keptNewerText,
        boxGone,
        attachmentsLost: imageIds.length > 0,
      });
      const toastId: string | number = (reason === null ? toast.info : toast.error)(title, {
        description,
        duration: Infinity,
        // No box is known to hold the message, so the toast carries it.
        ...(boxGone ? { action: copyUnstartedDraftAction(text, () => toastId) } : {}),
      });
      if (!boxGone) unstartedDraftToastShown = { toastId, sessionId: id };
      return;
    }
  }

  /**
   * The toast's "Copy message" for a draft whose pane is gone. The toast
   * stays up until the copy has worked, since it is the one place the
   * message can still be taken from.
   */
  function copyUnstartedDraftAction(text: string, toastId: () => string | number) {
    return {
      label: 'Copy message',
      onClick: (event: MouseEvent) => {
        event.preventDefault();
        writeClipboardText(text).then(
          () => toast.dismiss(toastId()),
          (err) => console.error('[SessionChatPane] Failed to copy the unstarted draft:', err)
        );
      },
    };
  }

  async function handleNoteFollowupClick() {
    if (!session || !noteInfo || sending || noteFollowupSending) return;
    const target: SendMessageTarget = { sessionId: session.id, branchId: branchId ?? null };
    const hasParsedNote = noteInfo.hasParsedNote;
    sending = true;
    noteFollowupSending = true;
    beginComposerAction();
    try {
      const prompt = await buildNoteFollowupMessage(
        target.sessionId,
        target.branchId,
        hasParsedNote
      );
      if (session?.id !== target.sessionId || (branchId ?? null) !== target.branchId) return;
      await sendMessage(prompt, undefined, true, target);
    } catch (e) {
      sendError = `Failed to send: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      noteFollowupSending = false;
      sending = false;
    }
  }

  async function handleDeleteQueuedMessage(id: string) {
    if (queueActionIds.has(id)) return;
    queueActionIds = new Set(queueActionIds).add(id);
    beginComposerAction();
    try {
      await deleteQueuedSessionMessage(id);
      await refreshQueuedMessages();
    } catch (e) {
      sendError = `Failed to remove queued message: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      const next = new Set(queueActionIds);
      next.delete(id);
      queueActionIds = next;
    }
  }

  async function handleSendQueuedMessage(id: string) {
    if (isLive || sending || queueActionIds.has(id)) return;
    // The backend doesn't claim queued follow-ups while disconnected, and
    // holds them while the project transfers.
    if (isDisconnected || transferring) return;
    queueActionIds = new Set(queueActionIds).add(id);
    beginComposerAction();
    try {
      await sendQueuedSessionMessage(id);
      await refreshQueuedMessages();
      if (session) session = { ...session, status: 'running' };
      // The queued message starts a new turn — see sendMessage.
      clearBackgroundHold();
      startPolling();
      scrollToBottom();
    } catch (e) {
      sendError = `Failed to send queued message: ${e instanceof Error ? e.message : String(e)}`;
      await refreshQueuedMessages();
    } finally {
      const next = new Set(queueActionIds);
      next.delete(id);
      queueActionIds = next;
    }
  }

  /** How long a Check now shows as pending without hearing back. */
  const CHECK_NOW_PENDING_MS = 10_000;

  /**
   * Ask the backend to reconcile this `disconnected` session now. Returns at
   * once; the checking state, and the outcome, arrive as events.
   */
  async function handleCheckNow() {
    if (!session) return;
    const id = session.id;
    beginComposerAction();
    checkNowRequest = { sessionId: id, stateAtRequest: reconcileState };
    if (checkNowTimer) clearTimeout(checkNowTimer);
    checkNowTimer = setTimeout(() => {
      checkNowTimer = null;
      if (checkNowRequest?.sessionId === id) checkNowRequest = null;
    }, CHECK_NOW_PENDING_MS);
    try {
      await reconcileSession(id);
    } catch (e) {
      if (checkNowRequest?.sessionId === id) checkNowRequest = null;
      sendError = `Failed to check the workstation: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  async function handleCancel() {
    if (!session || cancelling) return;
    cancelling = true;
    beginComposerAction();
    try {
      await cancelSession(session.id);
    } catch (e) {
      sendError = `Failed to cancel: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      cancelling = false;
    }
  }

  /**
   * Stop one background task; the session keeps waiting on the rest. The row
   * disappears via the hold report once the agent publishes the task's
   * terminal state, and the button stays in-flight until then — clearing the
   * mark when the request merely resolves would re-enable it for that gap,
   * which reads as the stop not having taken.
   *
   * A `false` answer means the agent stopped nothing (unknown id, already
   * finished, or a stop already in flight): un-mark the row and say so, since
   * no terminal state may be coming to explain the button re-enabling.
   */
  async function handleStopTask(taskId: string) {
    if (!session || stoppingTaskIds.has(taskId)) return;
    stoppingTaskIds = new Set(stoppingTaskIds).add(taskId);
    // A fresh click supersedes whatever the last one was told.
    clearTaskStopNotice(taskId);
    try {
      const stopped = await stopSessionAsyncTask(session.id, taskId);
      if (!stopped) {
        unmarkStoppingTask(taskId);
        noteTaskStopOutcome(
          taskId,
          "The agent didn't stop this task — it may have already finished."
        );
      }
    } catch (e) {
      unmarkStoppingTask(taskId);
      noteTaskStopOutcome(taskId, `Stop failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  /**
   * Report what a per-task stop answered, on the row it is about.
   *
   * Deliberately not `error`: that state replaces the entire transcript with a
   * centered error card until the next load or send, which is a heavy penalty
   * for the likeliest answer here (the task finished a moment before the
   * click). The notice lives on its row instead, and dies with it.
   *
   * Nothing is said at all once the row has left the reported hold: the task
   * terminated while the request was in flight — what the click asked for — so
   * a "did not stop" line would contradict the row vanishing in front of the
   * user.
   */
  function noteTaskStopOutcome(taskId: string, notice: string) {
    if (!isTaskHeld(backgroundHold, taskId)) return;
    taskStopNotices = new Map(taskStopNotices).set(taskId, notice);
  }

  /** Drop one row's stop notice. */
  function clearTaskStopNotice(taskId: string) {
    if (!taskStopNotices.has(taskId)) return;
    const next = new Map(taskStopNotices);
    next.delete(taskId);
    taskStopNotices = next;
  }

  /** Re-enable one task row's stop button. */
  function unmarkStoppingTask(taskId: string) {
    if (!stoppingTaskIds.has(taskId)) return;
    const next = new Set(stoppingTaskIds);
    next.delete(taskId);
    stoppingTaskIds = next;
  }

  // =========================================================================
  // Input handling
  // =========================================================================

  function handleInputKeydown(e: KeyboardEvent) {
    // Multiline input: plain Enter inserts a newline; Cmd/Ctrl+Enter sends,
    // but not while it commits an IME composition (see ChatComposer).
    if (isImeKeydown(e)) return;
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      handleSend();
    }
  }

  /** The draft changed: the last failure line has been read, so drop it. */
  function handleInput() {
    sendError = null;
  }

  // =========================================================================
  // Copy
  // =========================================================================

  async function copyContent(content: string, id: number | string) {
    try {
      await navigator.clipboard.writeText(content);
      copiedId = id;
      setTimeout(() => {
        if (copiedId === id) copiedId = null;
      }, 1500);
    } catch {
      // clipboard API may fail in some contexts
    }
  }

  // =========================================================================
  // Tool call expand/collapse
  // =========================================================================

  function toggleTool(key: string) {
    const next = new Set(expandedTools);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    expandedTools = next;
  }

  // =========================================================================
  // Helpers
  // =========================================================================

  /** Whether the user has intentionally scrolled up (disables auto-scroll). */
  let userScrolledUp = $state(false);
  let lastScrollTop = 0;
  /**
   * The first group in view, kept current from the scroll handler because
   * `park` usually runs after the dialog's DOM is gone and cannot measure. Only
   * meaningful while `userScrolledUp`: a pane following the bottom restores to
   * the bottom, so the rect reads are skipped there (that is the live case,
   * where appends scroll the box twice a second).
   */
  let scrollAnchor: ScrollAnchor | null = null;
  /** `scrollTop` when `scrollAnchor` was last captured. */
  let anchorScrollTop = NaN;
  /**
   * Scrolls shorter than this keep the previous anchor. Recapturing walks the
   * group list and probes layout offsets on every scroll event, the hottest
   * path during wheel scrolling; a restore that is a few pixels off is
   * invisible, so a trackpad's 1 px events are coalesced into one capture.
   */
  const ANCHOR_RECAPTURE_PX = 4;
  /**
   * How close to the end still counts as at the bottom. `clientHeight` rounds
   * a fractional box height and `scrollTop` may read truncated, so a box
   * scrolled to its true bottom can read a pixel short of it (615.27 tall:
   * restored to 2623 of a reported 2624); read as the user scrolling up, that
   * would stop the transcript following the bottom.
   */
  const AT_BOTTOM_TOLERANCE_PX = 2;

  function handleScroll() {
    if (!messagesEl) return;
    const { scrollTop, scrollHeight, clientHeight } = messagesEl;
    const atBottom = scrollHeight - scrollTop - clientHeight < AT_BOTTOM_TOLERANCE_PX;

    if (scrollTop < lastScrollTop && !atBottom) {
      // Scroll position moved upward — user scrolled up (any input method)
      userScrolledUp = true;
    }
    if (atBottom) {
      // They're back at bottom — re-enable auto-scroll
      userScrolledUp = false;
    }
    noteScrollPosition(messagesEl);
    if (scrollTop < LOAD_OLDER_THRESHOLD_PX) void loadOlderMessages();
  }

  /** Record where the box is after a scroll — the user's or one of ours — so
   *  `handleScroll` does not read our own jumps as the user scrolling up and
   *  `park` has an anchor to save. */
  function noteScrollPosition(el: HTMLElement) {
    lastScrollTop = el.scrollTop;
    if (!userScrolledUp) return;
    if (scrollAnchor && Math.abs(el.scrollTop - anchorScrollTop) < ANCHOR_RECAPTURE_PX) return;
    scrollAnchor = captureScrollAnchor(el);
    anchorScrollTop = el.scrollTop;
  }

  /**
   * Ask for the next page while the loaded rows do not overflow the viewport.
   * Only that case needs a programmatic request: once the transcript scrolls
   * at all, `handleScroll` applies `LOAD_OLDER_THRESHOLD_PX` on real scrolling.
   * Reusing the threshold here would fetch a second page, unasked, for every
   * session whose first page overflows by less than it.
   */
  function loadOlderMessagesIfUnscrollable() {
    if (messagesEl && messagesEl.scrollHeight <= messagesEl.clientHeight) {
      void loadOlderMessages();
    }
  }

  /**
   * Fetch the page of visible rows above `messages[0]` and prepend it, keeping
   * what the user is looking at where it is. One request at a time: the scroll
   * handler fires continuously near the top, and a second request for the same
   * `beforeId` would only return the same page.
   */
  async function loadOlderMessages() {
    if (!hasOlderMessages || loadingOlder || loading || closed) return;
    const el = messagesEl;
    const first = messages[0];
    if (!el || !first) return;
    const id = sessionId;
    loadingOlder = true;
    try {
      const rows = await getOlderSessionMessages(
        id,
        pageRequestLimit(TRANSCRIPT_PAGE_ROWS),
        first.id
      );
      // The pane may have moved on: another session, or a reload that
      // replaced the window this page was fetched for.
      if (closed || sessionId !== id || messages[0]?.id !== first.id) return;
      const page = transcriptPageFromRows(rows, TRANSCRIPT_PAGE_ROWS);
      // Anchor on the first rendered group: after the prepend it must sit at
      // the same viewport offset, so the scroll position moves by exactly the
      // height inserted above it. The keyed each re-creates that group when
      // the page extends the tool run it starts with; the container's growth
      // is the same quantity while every change is above the viewport.
      const anchor = el.querySelector<HTMLElement>('.transcript-group');
      const anchorTop = anchor?.getBoundingClientRect().top ?? 0;
      const heightBefore = el.scrollHeight;
      const resumeIntros = intro.suspend();
      messages = prependOlderMessages(messages, page.messages);
      hasOlderMessages = page.hasMore;
      await tick();
      resumeIntros();
      el.scrollTop += anchor?.isConnected
        ? anchor.getBoundingClientRect().top - anchorTop
        : el.scrollHeight - heightBefore;
      // That jump is ours, not the user's.
      noteScrollPosition(el);
    } catch (e) {
      // Left for the next scroll to retry rather than looping on a dead server.
      console.error('Failed to load earlier messages:', e);
      return;
    } finally {
      loadingOlder = false;
    }
    loadOlderMessagesIfUnscrollable();
  }

  /** Scroll to bottom unconditionally (e.g. initial load, user sends message).
   *  When afterSlide is true, also re-scrolls after the slide transition
   *  finishes so the new content isn't hidden below the fold. */
  function scrollToBottom(afterSlide = false) {
    tick().then(() => {
      if (messagesEl) {
        messagesEl.scrollTop = messagesEl.scrollHeight;
        userScrolledUp = false;
      }
      if (afterSlide) {
        setTimeout(() => {
          if (!userScrolledUp && messagesEl) {
            messagesEl.scrollTop = messagesEl.scrollHeight;
          }
        }, SLIDE_DURATION + 10);
      }
    });
  }

  /** Scroll to bottom only if the user hasn't intentionally scrolled up. */
  function scrollToBottomIfNear(afterSlide = false) {
    if (!userScrolledUp) {
      scrollToBottom(afterSlide);
    }
  }

  // The note follow-up row appears below the messages once the turn that
  // wrote the note ends. That isn't an append, and the row slides in inside
  // the messages list rather than as a row of the box itself, so the
  // observer above misses it too. It is followed like an append when it
  // appears: `afterSlide` re-pins once the slide has played. Only the row
  // appearing does this; whether the pane may scroll is read untracked.
  let noteFollowupShown = $derived(!!noteFollowupLabel);
  $effect(() => {
    if (!noteFollowupShown) return;
    untrack(() => {
      if (!active || closed || parkedScroll || deferredRestore.pending) return;
      scrollToBottomIfNear(true);
    });
  });

  // Keep a transcript that follows the bottom at the bottom when its own box
  // is resized: the full-screen dialog shrinking to the visual viewport as
  // the on-screen keyboard opens (and growing back as it closes), the
  // composer gaining a line, a window or dialog resize, the phone rotating.
  // Shrinking the box leaves `scrollTop` alone and narrowing it reflows the
  // messages taller; neither fires a scroll event, so neither `handleScroll`
  // nor the append path would re-pin it and the newest messages would end
  // up under the composer. A parked pane is left alone: its box is detached
  // or, just reattached, waiting for `unpark`'s deferred restore, which this
  // must not overwrite (observeSizeChanges already skips the 0×0 detached
  // box itself).
  //
  // Rows the session's state adds below the messages (an error or
  // end-of-session card, the disconnected alert) aren't appends, which is
  // what the append path follows, so they're followed here as they're added:
  // a card arriving with the keyboard up would otherwise sit below the
  // composer until the box was next resized.
  onMount(() => {
    const el = messagesEl;
    if (!el) return;
    const follow = () => {
      if (!active || closed || parkedScroll || deferredRestore.pending) return;
      if (userScrolledUp) return;
      el.scrollTop = el.scrollHeight;
      noteScrollPosition(el);
    };
    const stopResize = observeSizeChanges(el, follow);
    const rows =
      typeof MutationObserver === 'undefined'
        ? null
        : new MutationObserver((records) => {
            if (records.some((r) => [...r.addedNodes].some((n) => n.nodeType === 1))) follow();
          });
    rows?.observe(el, { childList: true });
    return () => {
      stopResize();
      rows?.disconnect();
    };
  });

  /** LRU cache for rendered markdown, keyed by source string. Live-transcript
   *  re-renders mostly re-request unchanged content, and each render is a full
   *  marked parse + DOMPurify sanitize (plus synchronous WASM renders for
   *  pikchr fences). Output is deterministic per content for a given renderer
   *  and hashtag context, so the cache resets when either changes. */
  const MARKDOWN_CACHE_LIMIT = 500;
  const markdownHtmlCache = new Map<string, string>();
  let markdownCacheRenderer: PikchrRenderer | null = null;
  let markdownCacheHashtagItems: HashtagItem[] | null = null;

  function renderMarkdown(content: string): string {
    // Read both reactive inputs unconditionally so cached calls keep the same
    // dependency tracking as uncached ones.
    const renderer = pikchrRenderer;
    const items = hashtagItems;
    if (markdownCacheRenderer !== renderer || markdownCacheHashtagItems !== items) {
      markdownHtmlCache.clear();
      markdownCacheRenderer = renderer;
      markdownCacheHashtagItems = items;
    }
    const cached = markdownHtmlCache.get(content);
    if (cached !== undefined) {
      markdownHtmlCache.delete(content);
      markdownHtmlCache.set(content, cached);
      return cached;
    }
    const html = renderSharedMarkdown(content, {
      pikchrRenderer: renderer,
      renderInlineText: items.length ? (text) => renderHashtagTokens(text, items) : undefined,
    });
    markdownHtmlCache.set(content, html);
    if (markdownHtmlCache.size > MARKDOWN_CACHE_LIMIT) {
      markdownHtmlCache.delete(markdownHtmlCache.keys().next().value!);
    }
    return html;
  }

  function openMediaViewerFromEvent(event: MouseEvent | KeyboardEvent): boolean {
    const media = getMarkdownMediaFromEvent(event.target);
    if (!media) return false;
    event.preventDefault();
    event.stopPropagation();
    mediaViewer = media;
    return true;
  }

  function openDiagramViewerFromEvent(event: MouseEvent | KeyboardEvent): boolean {
    const svgMarkup = getMarkdownDiagramSvgMarkup(event.target);
    if (!svgMarkup) return false;

    event.preventDefault();
    event.stopPropagation();
    diagramViewerSvg = svgMarkup;
    return true;
  }

  /** Memoized wrapper around the shared renderHashtagTokens. */
  const hashtagTokenCache = new Map<string, string>();
  let prevHashtagItems: HashtagItem[] | null = null;

  function renderHashtagTokens(text: string, items: HashtagItem[]): string {
    if (items !== prevHashtagItems) {
      hashtagTokenCache.clear();
      prevHashtagItems = items;
    }
    const cached = hashtagTokenCache.get(text);
    if (cached !== undefined) return cached;
    const result = renderHashtagTokensShared(text, items);
    hashtagTokenCache.set(text, result);
    return result;
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

  // =========================================================================
  // XML tag parsing (action / branch-history blocks)
  // =========================================================================

  type ContentSegment =
    | { type: 'text'; text: string }
    | { type: 'xml-block'; tag: string; label: string; content: string; icon: typeof Zap };

  /** Memoization caches for XML block detection and content segment parsing.
   *  Message content is immutable once set, so keying on the string is reliable. */
  const xmlBlocksCache = new Map<string, boolean>();
  const segmentsCache = new Map<string, ContentSegment[]>();

  function cachedHasXmlBlocks(content: string): boolean {
    const cached = xmlBlocksCache.get(content);
    if (cached !== undefined) return cached;
    const result = hasXmlBlocks(content);
    xmlBlocksCache.set(content, result);
    return result;
  }

  /** Parse content into segments, extracting XML-style tagged blocks. */
  function parseContentSegments(content: string): ContentSegment[] {
    const cached = segmentsCache.get(content);
    if (cached !== undefined) return cached;
    const segments: ContentSegment[] = [];
    let remaining = content;

    const tagPattern = new RegExp(`<(${XML_BLOCK_TAGS.join('|')})>([\\s\\S]*?)</\\1>`, 'g');
    let lastIndex = 0;
    let match: RegExpExecArray | null;

    while ((match = tagPattern.exec(remaining)) !== null) {
      // Text before this tag
      if (match.index > lastIndex) {
        const text = remaining.slice(lastIndex, match.index).trim();
        if (text) segments.push({ type: 'text', text });
      }

      const tag = match[1];
      const label =
        tag === 'action'
          ? 'Action instructions'
          : tag === 'branch-history'
            ? 'Branch history'
            : tag === 'pikchr-grammar'
              ? 'Pikchr grammar'
              : 'Launch context';
      const icon = tag === 'action' ? Zap : tag === 'pikchr-grammar' ? BookOpen : GitBranch;
      segments.push({ type: 'xml-block', tag, label, content: match[2].trim(), icon });

      lastIndex = match.index + match[0].length;
    }

    // Remaining text after last tag
    if (lastIndex < remaining.length) {
      const text = remaining.slice(lastIndex).trim();
      if (text) segments.push({ type: 'text', text });
    }

    // If no tags found, return single text segment
    if (segments.length === 0 && content.trim()) {
      segments.push({ type: 'text', text: content });
    }

    segmentsCache.set(content, segments);
    return segments;
  }

  /** Track which XML blocks are expanded */
  let expandedXmlBlocks = $state<Set<string>>(new Set());

  function toggleXmlBlock(key: string) {
    const next = new Set(expandedXmlBlocks);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    expandedXmlBlocks = next;
  }

  function publishSearchState() {
    onSearchStateChange?.({ matchCount, currentIndex: currentMatchIndex });
  }

  export function clearSearch() {
    searchQuery = '';
    if (messagesEl) {
      clearHighlights(messagesEl);
    }
    matchCount = 0;
    currentMatchIndex = 0;
    matchElements = [];
    publishSearchState();
  }

  export function performSearch(query: string) {
    searchQuery = query;
    if (!messagesEl) {
      publishSearchState();
      return;
    }

    // Clear previous highlights
    clearHighlights(messagesEl);
    matchElements = [];
    matchCount = 0;
    currentMatchIndex = 0;

    // If query is empty, nothing to highlight
    if (!query.trim()) {
      publishSearchState();
      return;
    }

    // Highlight matches
    const result = highlightMatches(messagesEl, query, currentMatchIndex);
    matchElements = result.elements;
    matchCount = result.total;

    // Scroll to first match if any
    if (matchElements.length > 0) {
      scrollToMatch(matchElements[0], messagesEl);
    }
    publishSearchState();
  }

  export function nextMatch() {
    if (isComposerFocused()) return;
    if (matchElements.length === 0) return;

    // Remove current class from old match
    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    // Cycle to next match (wrap around)
    currentMatchIndex = (currentMatchIndex + 1) % matchElements.length;

    // Add current class to new match
    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex], messagesEl);
    publishSearchState();
  }

  export function previousMatch() {
    if (isComposerFocused()) return;
    if (matchElements.length === 0) return;

    // Remove current class from old match
    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    // Cycle to previous match (wrap around)
    currentMatchIndex = (currentMatchIndex - 1 + matchElements.length) % matchElements.length;

    // Add current class to new match
    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex], messagesEl);
    publishSearchState();
  }

  // Re-apply search when messages change (for live sessions)
  $effect(() => {
    if (searchQuery.trim() && messagesEl) {
      // Debounce to avoid excessive highlighting during streaming
      const timer = setTimeout(() => {
        performSearch(searchQuery);
      }, 300);
      return () => clearTimeout(timer);
    }
  });

  $effect(() => {
    if (!active || !sessionHasPikchr || pikchrRenderer || pikchrRendererLoadFailed) return;

    const loadKey = pikchrRendererLoadKey;
    let stale = false;
    // The chat renders with the fenced source while the renderer loads, so
    // unlike NoteModal nothing is held blank here: wait well past the default
    // cap rather than latch the failed key on a slow load.
    loadPikchrRenderer(SESSION_PIKCHR_LOAD_TIMEOUT_MS)
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
    if (!active) {
      pikchrRendererLoadFailedKey = null;
    }
  });

  $effect(() => {
    const providerId = session?.provider ?? null;
    const workingDir = session?.workingDir ?? null;
    const hasMetadataConfig = metadataFollowupConfig !== null;

    if (!active || !providerId || hasMetadataConfig) {
      discoveredFollowupConfig = null;
      followupConfigLoading = false;
      followupConfigError = null;
      if (!active || !providerId) {
        modelSpecificFollowupConfig = null;
        modelSpecificFollowupModelValue = null;
        modelSpecificFollowupSessionId = null;
      }
      return;
    }

    const run = ++followupDiscoveryRun;
    let cancelled = false;
    followupConfigLoading = true;
    followupConfigError = null;

    discoverAcpConfig(providerId, workingDir)
      .then(({ data, revalidating }) => {
        if (cancelled || run !== followupDiscoveryRun) return;
        discoveredFollowupConfig = data;
        followupConfigLoading = false;
        revalidating
          ?.then((fresh) => {
            if (!cancelled && run === followupDiscoveryRun) {
              discoveredFollowupConfig = fresh;
            }
          })
          .catch((error) => {
            if (!cancelled && run === followupDiscoveryRun) {
              console.error('Failed to revalidate ACP config:', error);
            }
          });
      })
      .catch((error) => {
        if (cancelled || run !== followupDiscoveryRun) return;
        console.error('Failed to discover ACP config:', error);
        discoveredFollowupConfig = null;
        followupConfigLoading = false;
        followupConfigError = error instanceof Error ? error.message : String(error);
      });

    return () => {
      cancelled = true;
    };
  });

  $effect(() => {
    const key = selectorStateKey(
      session?.id,
      followupModelSelector,
      session?.acpConfigSelection?.model ?? null
    );
    if (key !== followupModelStateKey) {
      if (!(
        followupModelTouched && selectorHasValue(followupModelSelector, selectedFollowupModelValue)
      )) {
        selectedFollowupModelValue = initialSelectorValue(
          followupModelSelector,
          session?.acpConfigSelection?.model ?? null
        );
        followupModelTouchedSessionId = null;
      }
      followupModelStateKey = key;
    }
  });

  // Mirror of the model effect above: a touched selection survives selector
  // changes while it stays valid. While model-specific options load the
  // selector is null; the selection is retained as the desired value and
  // reconciled against the options once they arrive.
  $effect(() => {
    const key = selectorStateKey(
      session?.id,
      followupEffortSelector,
      session?.acpConfigSelection?.effort ?? null
    );
    if (key !== followupEffortStateKey) {
      if (
        followupEffortSelector &&
        !(
          followupEffortTouched &&
          selectorHasValue(followupEffortSelector, selectedFollowupEffortValue)
        )
      ) {
        selectedFollowupEffortValue = initialSelectorValue(
          followupEffortSelector,
          session?.acpConfigSelection?.effort ?? null
        );
        followupEffortTouchedSessionId = null;
      }
      followupEffortStateKey = key;
    }
  });

  export function close() {
    if (closed) return;
    closed = true;
    stopPolling();
  }

  /** Scroll state saved by `park` for the next `unpark`. */
  let parkedScroll: ParkedScroll | null = null;

  /** Applies a saved position to the live box; `unpark` schedules it. */
  const deferredRestore = createDeferredRestore((saved) => {
    const restored = applyParkedScroll(messagesEl ?? null, saved);
    if (!restored) return;
    lastScrollTop = restored.scrollTop;
    scrollAnchor = restored.anchor;
    anchorScrollTop = restored.scrollTop;
    // Back at the bottom it was following, so it follows again, whatever a
    // scroll event read while the box was detached or not yet restored (the
    // keyboard opening must re-pin it).
    if (saved.atBottom) userScrolledUp = false;
  });

  /**
   * Leave the document without being destroyed (`sessionPanePool`). The pool
   * calls this before it detaches the host, but the dialog above the host is
   * usually gone already: Svelte removes a destroyed block's DOM before it
   * runs the teardowns inside it, and WebKit resets `scrollTop` to 0 on
   * detach. So the position is read from the box only while it is still in
   * the document, and otherwise from `lastScrollTop` and `scrollAnchor`, which
   * `handleScroll` keeps current. A restore `unpark` scheduled but never got
   * to apply (parked again within the frame, or a window that never painted)
   * is the position to save: the box was never scrolled and the bookkeeping
   * never caught up. Search highlights are DOM state the next open must not
   * inherit, and the viewers portal to `#portal-root`, where nothing of a
   * parked pane may remain. `close` stops polling; the `active`-gated effects
   * (and PipelineSteps') drop the listeners once the pool clears `active`.
   */
  export function park() {
    const unapplied = deferredRestore.take();
    if (unapplied) {
      parkedScroll = unapplied;
    } else if (messagesEl) {
      const connected = messagesEl.isConnected;
      parkedScroll = {
        scrollTop: connected ? messagesEl.scrollTop : lastScrollTop,
        atBottom: !userScrolledUp,
        anchor: connected && userScrolledUp ? captureScrollAnchor(messagesEl) : scrollAnchor,
      };
    }
    clearSearch();
    mediaViewer = null;
    diagramViewerSvg = null;
    close();
  }

  /**
   * Back in the document. Runs synchronously once the host is reattached
   * inside the new dialog and before `active` flips, but the restore itself
   * waits for the next animation frame (see `createDeferredRestore`): read
   * and written here it forced style and layout over a dialog whose mount
   * flush was still adding DOM, and the frame paid them again. The frame
   * callback runs before that frame's own style and layout, so the first
   * paint shows the restored position and the forced layout is the one the
   * frame was about to do anyway. The anchored group is looked up in that
   * layout, so a box of another width (the NoteModal aside vs. the
   * SessionModal, a resized dialog) puts the same group back at the same
   * offset. `lastScrollTop` follows so `handleScroll` does not read the
   * restore as the user scrolling up or ask for an older page.
   */
  export function unpark() {
    const saved = parkedScroll;
    parkedScroll = null;
    if (!saved || !messagesEl) return;
    deferredRestore.schedule(saved);
  }

  function selectorStateKey(
    currentSessionId: string | undefined,
    selector: AcpConfigSelector | null,
    storedSelection: AcpConfigValueSelection | null
  ): string {
    const optionIds = selector?.options.map((option) => option.valueId).join(',') ?? '';
    return [
      currentSessionId ?? '',
      selector?.configId ?? '',
      selector?.currentValueId ?? '',
      optionIds,
      storedSelection?.configId ?? '',
      storedSelection?.valueId ?? '',
    ].join('\0');
  }

  function initialSelectorValue(
    selector: AcpConfigSelector | null,
    storedSelection: AcpConfigValueSelection | null
  ): string | null {
    if (
      storedSelection &&
      storedSelection.configId === selector?.configId &&
      selectorHasValue(selector, storedSelection.valueId)
    ) {
      return storedSelection.valueId;
    }
    return defaultSelectorValue(selector);
  }

  function shouldSendFollowupSelectorSelection(
    selector: AcpConfigSelector | null,
    valueId: string | null,
    storedSelection: AcpConfigValueSelection | null,
    touched: boolean
  ): boolean {
    if (!selector || !valueId) return false;
    if (touched) return true;
    if (!storedSelection) return false;

    const storedValueIsAvailable =
      storedSelection.configId === selector.configId &&
      selectorHasValue(selector, storedSelection.valueId);
    if (!storedValueIsAvailable) return true;

    return storedSelection.valueId === valueId;
  }

  function handleFollowupModelChange(value: string) {
    selectedFollowupModelValue = value;
    followupModelTouchedSessionId = session?.id ?? null;
    // The effort selection is kept as the desired value; the effort effect
    // reconciles it once the model-specific options arrive.
    modelSpecificFollowupConfig = null;
    modelSpecificFollowupModelValue = value;
    modelSpecificFollowupSessionId = session?.id ?? null;

    const providerId = session?.provider ?? null;
    if (providerId) {
      setAcpConfigPref(providerId, { model: value });
    }
    const workingDir = session?.workingDir ?? null;
    if (!active || !providerId) return;

    const run = ++followupDiscoveryRun;
    followupConfigLoading = true;
    followupConfigError = null;

    discoverAcpConfig(providerId, workingDir, { selectedModelValue: value })
      .then(({ data, revalidating }) => {
        if (run !== followupDiscoveryRun) return;
        modelSpecificFollowupConfig = data;
        followupConfigLoading = false;
        revalidating
          ?.then((fresh) => {
            if (run === followupDiscoveryRun) {
              modelSpecificFollowupConfig = fresh;
            }
          })
          .catch((error) => {
            if (run === followupDiscoveryRun) {
              console.error('Failed to revalidate ACP config for selected model:', error);
            }
          });
      })
      .catch((error) => {
        if (run !== followupDiscoveryRun) return;
        console.error('Failed to discover ACP config for selected model:', error);
        followupConfigLoading = false;
        followupConfigError = error instanceof Error ? error.message : String(error);
        // Fall back to the non-model-specific effort options so the effort
        // column survives a transient discovery failure. The model choice is
        // kept; if the effort sent with it turns out to be stale for the new
        // model, the driver skips it and uses the provider default.
        modelSpecificFollowupModelValue = null;
      });
  }

  function handleFollowupEffortChange(value: string) {
    selectedFollowupEffortValue = value;
    followupEffortTouchedSessionId = session?.id ?? null;
    if (session?.provider) {
      setAcpConfigPref(session.provider, {
        effort: value,
        effortModel: selectedFollowupModelValue ?? undefined,
      });
    }
  }

  let slashCommands = $derived.by(() => latestAvailableCommands(acpMetadataMessages));
  let slashQuery = $derived.by(() => {
    // A command starts the message; with quotes above it the input doesn't.
    if (draftQuotes.length > 0) return null;
    const trimmed = inputText.trimStart();
    return trimmed.startsWith('/') ? trimmed.slice(1).toLowerCase() : null;
  });
  let matchingSlashCommands = $derived.by(() => {
    if (slashQuery === null) return [];
    return slashCommands
      .filter((command) => command.name.toLowerCase().includes(slashQuery))
      .slice(0, 6);
  });

  /** Note-indicator scans keyed on the message object: rows keep identity
   *  across polls except the re-fetched tail, so stale entries self-evict. */
  const noteIndicatorCache = new WeakMap<SessionMessage, boolean>();

  function messageHasNoteIndicator(message: SessionMessage): boolean {
    let hasNote = noteIndicatorCache.get(message);
    if (hasNote === undefined) {
      hasNote = splitAtNoteIndicator(message.content).hasNote;
      noteIndicatorCache.set(message, hasNote);
    }
    return hasNote;
  }

  let noteBearingMessageIds = $derived.by(() => {
    return messages
      .filter((message) => message.role === 'assistant' && messageHasNoteIndicator(message))
      .map((message) => message.id);
  });

  let isPipelineRunning = $derived(isLive && !!session?.pipeline);

  function insertSlashCommand(command: AcpCommand) {
    inputText = `/${command.name}${command.inputHint ? ' ' : ''}`;
    tick().then(() => inputEl?.focus());
  }

  function acpEventSummary(event: AcpTranscriptEvent): string {
    if (event.kind === 'available_commands_update') {
      const commands = arrayProp(event.content, 'availableCommands');
      return commands.length === 1 ? '1 command' : `${commands.length} commands`;
    }
    if (event.kind === 'config_options_update') {
      const options = Array.isArray(event.content) ? event.content : [];
      return options.length === 1 ? '1 option' : `${options.length} options`;
    }
    return compactJsonSummary(event.content);
  }

  function configOptions(content: unknown): Record<string, unknown>[] {
    return Array.isArray(content)
      ? content.filter(
          (item): item is Record<string, unknown> =>
            !!item && typeof item === 'object' && !Array.isArray(item)
        )
      : [];
  }

  function availableCommands(content: unknown): AcpCommand[] {
    const commands = arrayProp(content, 'availableCommands');
    return commands
      .map((command) => {
        const name = stringProp(command, 'name');
        if (!name) return null;
        return {
          name,
          description: stringProp(command, 'description') ?? '',
          inputHint: stringProp(objectProp(command, 'input'), 'hint'),
        };
      })
      .filter((command): command is AcpCommand => command !== null);
  }

  function optionCurrentValue(option: Record<string, unknown>): string {
    const currentValue = stringProp(option, 'currentValue');
    if (currentValue) return currentValue;
    const select = objectProp(option, 'select');
    return stringProp(select, 'currentValue') ?? '';
  }

  function optionChoices(option: Record<string, unknown>): { value: string; name: string }[] {
    const options = arrayProp(option, 'options');
    if (options.length === 0) return [];
    if (options.every((item) => arrayProp(item, 'options').length > 0)) {
      return options.flatMap((group) => optionChoices(group));
    }
    return options
      .map((item) => {
        const value = stringProp(item, 'value');
        const name = stringProp(item, 'name') ?? value;
        return value && name ? { value, name } : null;
      })
      .filter((item): item is { value: string; name: string } => item !== null);
  }

  function stringProp(value: unknown, key: string): string | null {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
    const prop = (value as Record<string, unknown>)[key];
    return typeof prop === 'string' ? prop : null;
  }

  function objectProp(value: unknown, key: string): Record<string, unknown> | null {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
    const prop = (value as Record<string, unknown>)[key];
    return prop && typeof prop === 'object' && !Array.isArray(prop)
      ? (prop as Record<string, unknown>)
      : null;
  }

  function arrayProp(value: unknown, key: string): Record<string, unknown>[] {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
    const prop = (value as Record<string, unknown>)[key];
    return Array.isArray(prop)
      ? prop.filter(
          (item): item is Record<string, unknown> =>
            !!item && typeof item === 'object' && !Array.isArray(item)
        )
      : [];
  }

  function compactJsonSummary(value: unknown): string {
    const text = formatJson(value).replace(/\s+/g, ' ').trim();
    return text.length > 72 ? `${text.slice(0, 72)}…` : text;
  }

  /** Slide-in transition that is suppressed during the initial load. */
  function messageSlide(node: Element) {
    if (!intro.animate) return { duration: 0 };
    return slide(node, { duration: SLIDE_DURATION });
  }
</script>

<svelte:window onpaste={handleImagePaste} />

<!-- Lives here rather than in ToolCallCard because renderMarkdown closes over the
     pane's pikchr renderer state; passed to ToolCallCard as its diagram snippet. -->
{#snippet pikchrDiagram(source: string)}
  <div class="tool-diagram markdown-content">
    {@html renderMarkdown(fencedDiagramMarkdown('pikchr', source))}
  </div>
{/snippet}

<div
  bind:this={modalElement}
  class={`session-chat-pane ${compact ? 'compact' : ''} ${dragOver ? 'drag-over' : ''}`}
>
  <!-- Latest plan, pinned above the scrollable transcript. Keyed on the
       session so the expanded/collapsed toggle resets when this pane is
       reused for a different session. -->
  {#if plan}
    {#key session?.id}
      <PlanCard entries={plan} defaultExpanded={!viewport.isMobile && !compact} />
    {/key}
  {/if}

  <!-- Messages area -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="modal-content"
    bind:this={messagesEl}
    onscroll={handleScroll}
    onclick={handleContentClick}
    onkeydown={handleContentKeydown}
  >
    {#if session?.pipeline}
      <PipelineSteps {sessionId} pipeline={session.pipeline} {active} />
    {/if}
    {#if loading}
      <div class="center-state">
        <Spinner size={24} />
        <span>Loading session…</span>
      </div>
    {:else if error}
      <div class="center-state error">
        <AlertCircle size={24} />
        <span>{error}</span>
      </div>
    {:else if grouped.length === 0 && isLive && !session?.pipeline}
      <div class="center-state">
        <Spinner size={20} />
        <span>Waiting for response…</span>
      </div>
    {:else if grouped.length === 0 && !session?.pipeline}
      <div class="center-state">
        <span>No messages</span>
      </div>
    {:else if grouped.length === 0}
      <!-- Pipeline is present but no messages yet — pipeline steps are the entire view -->
    {:else}
      <div class="messages">
        <!-- Sessions stagedctl started on behalf of another session or a
             connected app say so above the transcript; UI starts show nothing. -->
        <StartedByLabel startedBy={session?.startedBy} />
        {#if hasOlderMessages}
          <!-- Fixed-height slot, so the spinner appearing does not move the
               transcript below it. -->
          <div class="load-older" aria-live="polite">
            {#if loadingOlder}
              <Spinner size={14} />
              <span>Loading earlier messages…</span>
            {/if}
          </div>
        {/if}
        {#each grouped as group, groupIdx (transcriptGroupKey(group))}
          <div
            in:messageSlide
            class={group.type === 'user' ? 'transcript-group user-group' : 'transcript-group'}
            data-group-key={transcriptGroupKey(group)}
          >
            {#if group.type === 'user'}
              {@const hasBlocks = cachedHasXmlBlocks(group.message.content)}
              {@const segments = hasBlocks ? parseContentSegments(group.message.content) : []}
              {@const userText = hasBlocks
                ? segments
                    .filter((s) => s.type === 'text')
                    .map((s) => s.text)
                    .join('\n')
                : group.message.content}
              {@const xmlBlocks = segments.filter((s) => s.type === 'xml-block')}
              <!-- Context cards (above the bubble, left-aligned like tool calls) -->
              {#if xmlBlocks.length > 0}
                <div class="message-row context-block-group">
                  {#each xmlBlocks as seg, segIdx}
                    {#if seg.type === 'xml-block'}
                      {@const blockKey = `${group.message.id}-${segIdx}`}
                      {@const isOpen = expandedXmlBlocks.has(blockKey)}
                      <div class="context-card">
                        <!-- svelte-ignore a11y_click_events_have_key_events -->
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <div class="context-card-header" onclick={() => toggleXmlBlock(blockKey)}>
                          <span class="context-chevron">
                            {#if isOpen}
                              <ChevronDown size={12} />
                            {:else}
                              <ChevronRight size={12} />
                            {/if}
                          </span>
                          <seg.icon size={12} class="context-icon" />
                          <span class="context-label">{seg.label}</span>
                        </div>
                        {#if isOpen}
                          <div class="context-card-content">
                            <pre>{seg.content}</pre>
                          </div>
                        {/if}
                      </div>
                    {/if}
                  {/each}
                </div>
              {/if}
              <!-- User prompt bubble -->
              {#if userText.trim() || (group.message.imageIds && group.message.imageIds.length > 0)}
                <div class="message-row human-message">
                  <div class="group/bubble human-bubble">
                    {#if userText.trim()}
                      {@const userParts = parseQuotedMessage(userText.trim())}
                      {#if hasQuotedParts(userParts)}
                        <div class="human-quoted">
                          {#each userParts as part, partIdx (partIdx)}
                            {#if part.type === 'quote'}
                              <QuoteExcerpt quote={part.quote} headings={part.headings} />
                            {:else}
                              <span class="human-text"
                                >{@html renderHashtagTokens(part.text, hashtagItems)}</span
                              >
                            {/if}
                          {/each}
                        </div>
                      {:else}
                        <span class="human-text"
                          >{@html renderHashtagTokens(userText.trim(), hashtagItems)}</span
                        >
                      {/if}
                    {/if}
                    {#if group.message.imageIds && group.message.imageIds.length > 0}
                      <div class="message-images">
                        {#each group.message.imageIds as imgId}
                          <div class="message-image-thumb">
                            <AttachmentThumb
                              preview={messageImageCache.get(imgId)}
                              alt="attachment"
                              fit="natural"
                            />
                          </div>
                        {/each}
                      </div>
                    {/if}
                    <Button
                      variant="ghost"
                      size="icon"
                      class="message-copy-action touch-hit absolute top-1.5 right-1.5 size-auto rounded p-[3px] text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-hover)] hover:text-foreground group-hover/bubble:opacity-100 [&_svg]:!size-3"
                      title={copiedId === group.message.id ? 'Copied!' : 'Copy message'}
                      aria-label={copiedId === group.message.id ? 'Copied!' : 'Copy message'}
                      onclick={() => copyContent(group.message.content, group.message.id)}
                    >
                      {#if copiedId === group.message.id}
                        <Check size={12} />
                      {:else}
                        <Copy size={12} />
                      {/if}
                    </Button>
                  </div>
                </div>
              {/if}
            {:else if group.type === 'assistant'}
              {@const split = splitAtNoteIndicator(group.message.content, {
                streaming: isLive && groupIdx === lastGroupIndex,
              })}
              {@const firstNoteMessageId = noteBearingMessageIds[0]}
              {@const hasPreamble = split.preamble.trim().length > 0}
              {@const showAssistantCopy = hasPreamble || !split.hasNote}
              <div class="message-row assistant-message">
                <div
                  class="group/assistant assistant-content"
                  class:assistant-content-has-copy={showAssistantCopy}
                >
                  {#if hasPreamble}
                    <div class="markdown-content">
                      {@html renderMarkdown(split.preamble)}
                    </div>
                  {/if}
                  {#if split.hasNote}
                    <NoteActivityCard
                      label={noteActivityLabel({
                        isLive,
                        isLastGroup: groupIdx === lastGroupIndex,
                        isFirstNoteMessage: group.message.id === firstNoteMessageId,
                      })}
                    />
                  {/if}
                  {#if showAssistantCopy}
                    <Button
                      variant="ghost"
                      size="icon"
                      class="message-copy-action touch-hit absolute top-0 right-0 size-auto rounded p-[3px] text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-hover)] hover:text-foreground group-hover/assistant:opacity-100 [&_svg]:!size-3"
                      title={copiedId === group.message.id ? 'Copied!' : 'Copy message'}
                      aria-label={copiedId === group.message.id ? 'Copied!' : 'Copy message'}
                      onclick={() => copyContent(group.message.content, group.message.id)}
                    >
                      {#if copiedId === group.message.id}
                        <Check size={12} />
                      {:else}
                        <Copy size={12} />
                      {/if}
                    </Button>
                  {/if}
                </div>
              </div>
            {:else if group.type === 'tools'}
              <div class="message-row tool-group">
                {#each groupRichToolsByVerb(group.items) as verbGroup (verbGroup.key)}
                  {#if verbGroup.items.length === 1}
                    <ToolCallCard
                      item={verbGroup.items[0]}
                      {displayRoots}
                      expanded={expandedTools.has(verbGroup.items[0].key)}
                      slideDuration={SLIDE_DURATION}
                      onToggle={toggleTool}
                      {onOpenSession}
                      diagram={pikchrDiagram}
                    />
                  {:else}
                    {@const isGroupExpanded = expandedTools.has(verbGroup.key)}
                    <ToolCallHeader
                      verb={verbGroup.verb}
                      detail={verbGroup.summary}
                      statusTone={verbGroup.statusTone}
                      expanded={isGroupExpanded}
                      onToggle={() => toggleTool(verbGroup.key)}
                    />
                    {#if isGroupExpanded}
                      <div transition:slide={{ duration: SLIDE_DURATION }}>
                        {#each verbGroup.items as item (item.key)}
                          <ToolCallCard
                            {item}
                            {displayRoots}
                            nested
                            expanded={expandedTools.has(item.key)}
                            slideDuration={SLIDE_DURATION}
                            onToggle={toggleTool}
                            {onOpenSession}
                            diagram={pikchrDiagram}
                          />
                        {/each}
                      </div>
                    {/if}
                  {/if}
                {/each}
              </div>
            {:else}
              {@const event = group.event}
              {@const isExpanded = expandedTools.has(`event:${event.id}`)}
              <div class="message-row acp-event-row">
                <div class="acp-event-card">
                  <!-- svelte-ignore a11y_click_events_have_key_events -->
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <div class="acp-event-header" onclick={() => toggleTool(`event:${event.id}`)}>
                    <span class="tool-caret" class:tool-caret-expanded={isExpanded}>›</span>
                    <span class="acp-event-title">{event.title}</span>
                    <span class="tool-args-preview">{acpEventSummary(event)}</span>
                  </div>
                  {#if isExpanded}
                    <div class="acp-event-body" transition:slide={{ duration: SLIDE_DURATION }}>
                      {#if event.kind === 'config_options_update'}
                        <div class="config-list">
                          {#each configOptions(event.content) as option}
                            <label class="config-option">
                              <span>{stringProp(option, 'name')}</span>
                              <select disabled value={optionCurrentValue(option)}>
                                {#each optionChoices(option) as choice}
                                  <option value={choice.value}>{choice.name}</option>
                                {/each}
                              </select>
                            </label>
                          {/each}
                        </div>
                      {:else if event.kind === 'available_commands_update'}
                        <div class="command-list">
                          {#each availableCommands(event.content) as command}
                            <button class="command-row" onclick={() => insertSlashCommand(command)}>
                              <span>/{command.name}</span>
                              <small>{command.description}</small>
                            </button>
                          {/each}
                        </div>
                      {:else}
                        <pre>{formatJson(event.content)}</pre>
                      {/if}
                    </div>
                  {/if}
                </div>
              </div>
            {/if}
          </div>
        {/each}

        {#if isLive}
          <div
            class="thinking"
            class:thinking-waiting={activityRow.waitingOnBackground}
            in:messageSlide
          >
            <Spinner size={14} />
            <span>{activityRow.label}</span>
            <Button
              variant="destructive"
              size="sm"
              class="ml-auto"
              title={activityRow.stopLabel}
              aria-label={activityRow.stopLabel}
              onclick={handleCancel}
              disabled={cancelling}
            >
              {#if cancelling}
                <Spinner size={14} />
              {:else}
                <CircleStop size={14} />
              {/if}
              Stop
            </Button>
          </div>
          {#each holdTaskRows as taskRow (taskRow.id)}
            {@const stopNotice = taskStopNotices.get(taskRow.id)}
            <div class="thinking thinking-waiting hold-task-row" in:messageSlide>
              <span title={taskRow.description ?? undefined}>{taskRow.label}</span>
              <Button
                variant="outline"
                size="sm"
                class="ml-auto"
                title={taskRow.stopLabel}
                aria-label={taskRow.stopLabel}
                onclick={() => handleStopTask(taskRow.id)}
                disabled={stoppingTaskIds.has(taskRow.id)}
              >
                {#if stoppingTaskIds.has(taskRow.id)}
                  <Spinner size={14} />
                {:else}
                  <CircleStop size={14} />
                {/if}
                Stop task
              </Button>
            </div>
            <!-- Mounted with its row, empty until there is something to say:
                 `role="status"` announces *changes* to a region already in the
                 accessibility tree, so a region created in the same render as
                 its text is the one screen readers miss — which would be
                 exactly the answer the user clicked Stop for. -->
            <div class="hold-task-notice" class:hold-task-notice-idle={!stopNotice} role="status">
              {stopNotice ?? ''}
            </div>
          {/each}
        {/if}

        {#if noteFollowupLabel}
          <div class="note-followup-row" in:messageSlide>
            <Button
              variant="outline"
              size="sm"
              class="h-auto gap-1.5 rounded-md border-[var(--border-muted)] bg-[var(--note-bg)] px-3 py-1.5 text-xs font-medium text-[var(--note-color)] shadow-none hover:border-[var(--note-color)] hover:bg-[var(--note-bg-emphasis)] hover:text-[var(--note-color)] disabled:opacity-65"
              onclick={handleNoteFollowupClick}
              disabled={sending}
            >
              {#if noteFollowupSending}
                <Spinner size={13} />
              {:else}
                <FileText size={13} />
              {/if}
              <span>{noteFollowupLabel}</span>
            </Button>
          </div>
        {/if}
      </div>
    {/if}

    <!-- Cancelled sessions normally carry no message; one appears when the run
         was killed from outside with a recorded reason (e.g. a Pikchr child
         session whose generate_pikchr call timed out) and reads as an error. -->
    {#if (session?.status === 'error' || session?.status === 'cancelled') && session.errorMessage && !isReconcileCancelNotice(session.status, session.errorMessage)}
      {@const authError = isAuthenticationError(session.errorMessage)}
      <Alert.Root variant="destructive" class="mt-3">
        <AlertCircle />
        <Alert.Description>{sessionErrorMessage(session.errorMessage)}</Alert.Description>
        {#if authError}
          <Alert.Action>
            <!-- `Log in` leads when it is offered: this failure came from the
                 agent itself, and the Doctor panel behind `Fix` may well call the
                 check passing (its probe can't see an expired token), so `Fix`
                 is the fallback, not the answer. -->
            <div class="auth-actions">
              {#if login.canLogin}
                <Button variant="default" size="xs" disabled={login.running} onclick={login.start}>
                  {login.running ? 'Logging in…' : 'Log in'}
                </Button>
              {/if}
              <Button variant="outline" size="xs" onclick={() => openSettings('doctor')}>
                Fix
              </Button>
            </div>
          </Alert.Action>
        {/if}
      </Alert.Root>
      <AgentLoginPrompt checkId={login.checkId} />
    {:else if session && isDisconnected}
      <!-- Not live, so the thinking row's Stop is gone; without this the row
           would hold its branch busy with no way to end it. No Resume: a new
           turn would run beside the one that may still be going remotely. -->
      <DisconnectedSessionAlert
        reconcile={reconcileState}
        checkRequested={checkNowPending}
        stopping={cancelling}
        {userScrolledUp}
        reveal={(el) => revealWithin(el, messagesEl, { block: 'nearest' })}
        onCheckNow={handleCheckNow}
        onStop={handleCancel}
      />
    {:else if session && session.status !== 'running' && session.status !== 'queued'}
      {#if isResumableReason(session.completionReason)}
        {@const isRateLimited = session.completionReason === 'rate_limited'}
        {@const isWarning =
          isRateLimited ||
          session.completionReason === 'crashed' ||
          session.completionReason === 'remote_agent_lost' ||
          session.completionReason === 'app_quit' ||
          isReconcileCancelNotice(session.status, session.errorMessage)}
        <Alert.Root class="mt-3">
          <Info class={isWarning ? 'text-[var(--ui-warning)]' : 'text-[var(--text-muted)]'} />
          <Alert.Description>
            {sessionEndMessage(session)}
          </Alert.Description>
          <Alert.Action>
            <Button
              variant="outline"
              size="xs"
              onclick={() => sendMessage('Continue where you left off.')}
              disabled={sending}
            >
              {isRateLimited ? 'Resume now' : 'Resume'}
            </Button>
          </Alert.Action>
        </Alert.Root>
      {:else if isReconcileCancelNotice(session.status, session.errorMessage)}
        <Alert.Root class="mt-3">
          <Info class="text-[var(--ui-warning)]" />
          <Alert.Description>{sessionEndMessage(session)}</Alert.Description>
        </Alert.Root>
      {:else if reconnectedNote}
        <Alert.Root class="mt-3">
          <CloudCheck class="text-[var(--text-muted)]" />
          <Alert.Description>{reconnectedNote}</Alert.Description>
        </Alert.Root>
      {/if}
    {/if}
  </div>

  <!-- Input area with queued messages and image previews -->
  <div class="input-wrapper">
    {#if isPipelineRunning && grouped.length === 0}
      <!-- Stands in for the composer, so it shows the composer's failure
           line too: a failed Stop has nowhere else to appear. -->
      <div class="pipeline-stop-area">
        <ComposerError message={sendError} />
        <span class="inline-flex pipeline-stop-button" title="Stop workflow">
          <Button
            variant="destructive"
            size="sm"
            aria-label="Stop workflow"
            onclick={handleCancel}
            disabled={cancelling}
          >
            {#if cancelling}
              <Spinner size={14} />
            {:else}
              <CircleStop size={14} />
            {/if}
            Stop
          </Button>
        </span>
      </div>
    {:else}
      {#if hasQueuedMessages}
        <div class="queue-popover">
          {#each queuedMessages as msg (msg.id)}
            <div class="queue-item">
              <span class="queue-item-label"
                >{msg.status === 'sending'
                  ? 'Sending'
                  : transferring
                    ? QUEUED_FOR_TRANSFER_LABEL
                    : 'Queued'}</span
              >
              <div class="queue-item-body">
                <span class="queue-item-text">{msg.content}</span>
                {#if msg.imageIds.length > 0}
                  <div class="queue-item-images">
                    {#each msg.imageIds as imageId}
                      <span class="queue-item-image">
                        <AttachmentThumb
                          preview={messageImageCache.get(imageId)}
                          alt="queued attachment"
                          iconSize={12}
                        />
                      </span>
                    {/each}
                  </div>
                {/if}
                {#if msg.lastError}
                  <span class="queue-item-error">{msg.lastError}</span>
                {/if}
              </div>
              {#if !isLive && !isDisconnected && !transferring}
                <Button
                  variant="accent"
                  size="xs"
                  class="shrink-0"
                  title="Send queued message"
                  aria-label="Send queued message"
                  disabled={msg.status !== 'queued' || queueActionIds.has(msg.id)}
                  onclick={() => handleSendQueuedMessage(msg.id)}
                >
                  {#if queueActionIds.has(msg.id)}
                    <Spinner size={12} />
                  {:else}
                    Send
                  {/if}
                </Button>
              {/if}
              <Button
                variant="ghost"
                size="icon"
                class="size-[18px] shrink-0 rounded text-[var(--text-faint)] hover:bg-[var(--bg-hover)] hover:text-destructive [&_svg]:!size-2.5"
                title="Remove from queue"
                aria-label="Remove from queue"
                disabled={msg.status !== 'queued' || queueActionIds.has(msg.id)}
                onclick={() => handleDeleteQueuedMessage(msg.id)}
              >
                <X size={10} />
              </Button>
            </div>
          {/each}
        </div>
      {/if}
      {#if matchingSlashCommands.length > 0 && !isLive}
        <div class="slash-command-popover">
          {#each matchingSlashCommands as command}
            <button class="slash-command-item" onclick={() => insertSlashCommand(command)}>
              <span>/{command.name}</span>
              <small>{command.description || command.inputHint}</small>
            </button>
          {/each}
        </div>
      {/if}
      <ChatComposer
        bind:this={composerView}
        bind:textareaEl={inputEl}
        bind:value={inputText}
        bind:quotes={draftQuotes}
        onRemoveQuote={removeQuote}
        {sendControl}
        {hashtagItems}
        onkeydown={handleInputKeydown}
        oninput={handleInput}
        {canAttachImages}
        attachedImageIds={replyImageIds}
        {imagePreviews}
        onAttachFiles={handleAttachFiles}
        onRemoveImage={removeReplyImage}
        {attachError}
        {isLive}
        sendMode={preferences.composerSendMode}
        onSendModeChange={setComposerSendMode}
        {sending}
        sessionBlock={composerBlock}
        {sendError}
        onSend={handleSend}
        onStartNewSession={onStartNewSession ? handleStartNewSession : undefined}
        {newSessionDisabled}
      >
        {#snippet configPicker()}
          <AcpFixedConfigPicker
            providerId={session?.provider ?? null}
            providerLabel={followupProviderLabel}
            modelSelector={followupModelSelector}
            effortSelector={followupEffortSelector}
            selectedModelValue={selectedFollowupModelValue}
            selectedEffortValue={selectedFollowupEffortValue}
            loading={followupConfigLoading}
            error={followupConfigError}
            disabled={isLive || sending}
            dropUp
            triggerClass={composerControlClass}
            layout="vertical"
            onModelChange={handleFollowupModelChange}
            onEffortChange={handleFollowupEffortChange}
          />
        {/snippet}
      </ChatComposer>
    {/if}
  </div>
</div>

{#if mediaViewer}
  <MediaViewerModal
    open={true}
    src={resolveStagedMediaUrl(mediaViewer.id, mediaViewer.ext)}
    title={mediaViewer.alt}
    onClose={() => (mediaViewer = null)}
  />
{/if}

<DiagramViewerModal
  open={diagramViewerSvg !== null}
  svgMarkup={diagramViewerSvg}
  onClose={() => (diagramViewerSvg = null)}
/>

<style>
  /* ======================================================================= */
  /* Modal shell                                                             */
  /* ======================================================================= */

  /* When its rows don't all fit, the pane scrolls rather than clip,
     anchored at its foot: the message box and Send stay in view, and what
     sits above them (the plan, a queued message, the slash-command list,
     attachments, an error line) is a scroll away. The keyboard up on a phone
     held sideways leaves 48px (dialog-content's `cramped`); a long queue or
     slash-command list can overflow a short window too. `column-reverse`
     does the anchoring: a scroller starts at a reversed column's
     main-start, the bottom, and holds that edge while content above it
     grows or goes. The `order`s on the transcript and the input area put
     the rows back in their usual top-to-bottom order. While everything fits
     the transcript takes up the slack, so the rows sit just as in a plain
     column. When it doesn't, the transcript keeps whatever height is left,
     but never less than its own padding (24px in the mobile layout), and its
     own scroll position is untouched. */
  .session-chat-pane {
    display: flex;
    flex-direction: column-reverse;
    flex: 1;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
    overscroll-behavior: contain;
    border: 2px solid transparent;
    background: var(--bg-primary);
    transition:
      border-color 0.12s ease,
      background-color 0.12s ease;
  }

  .session-chat-pane.drag-over {
    border-color: var(--ui-accent);
    background: color-mix(in srgb, var(--ui-accent) 5%, var(--bg-chrome));
  }

  .session-chat-pane.compact .modal-content {
    padding: 12px;
    padding-left: calc(12px + var(--dialog-safe-left, 0px));
    padding-right: calc(12px + var(--dialog-safe-right, 0px));
  }

  .session-chat-pane.compact .messages {
    gap: 10px;
  }

  .session-chat-pane.compact .human-bubble {
    max-width: 92%;
  }

  /* ----- Content area (scrollable) --------------------------------------- */

  .modal-content {
    order: -1;
    flex: 1;
    overflow-x: hidden;
    overflow-y: auto;
    /* Keep scroll momentum inside the transcript so it doesn't chain to the
       page behind the keyboard-shrunk full-screen dialog. */
    overscroll-behavior: contain;
    /* Older pages are prepended with the scroll position corrected by hand
       (`loadOlderMessages`); the browser's own anchoring would correct it a
       second time. */
    overflow-anchor: none;
    padding: 16px;
    /* The transcript spans a full-screen dialog's width, so a swipe over
       the side insets still scrolls it; its content clears them
       (`--dialog-safe-left` / `--dialog-safe-right`, see dialog-content). */
    padding-left: calc(16px + var(--dialog-safe-left, 0px));
    padding-right: calc(16px + var(--dialog-safe-right, 0px));
    min-height: 0;
    background: var(--bg-primary);
  }

  /* Custom scrollbar */
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

  /* ----- Center states --------------------------------------------------- */

  .center-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 40px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    height: 100%;
  }

  .center-state.error {
    color: var(--ui-danger);
  }

  /* ----- Messages -------------------------------------------------------- */

  .messages {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .load-older {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 24px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .message-row {
    position: relative;
  }

  /* Human messages — bubble, right-aligned */
  .human-message {
    display: flex;
    justify-content: flex-end;
  }

  .human-bubble {
    position: relative;
    max-width: 80%;
    padding: 8px 12px;
    padding-right: 30px;
    background: var(--bg-elevated, var(--bg-hover));
    border-radius: 14px 14px 4px 14px;
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.5;
    word-break: break-word;
    overflow: hidden;
  }

  .human-text {
    display: block;
    white-space: pre-wrap;
  }

  /* A reply built from note quotes: each quote, then the text around it. */
  .human-quoted {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .human-quoted :global(.quote-excerpt + .quote-excerpt) {
    margin-top: 4px;
  }

  /* Images attached to user messages */
  .message-images {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 6px;
  }

  /* 120px wide at the image's own aspect, cropped only past 120px tall. */
  .message-image-thumb {
    --attachment-thumb-max-height: 120px;
    position: relative;
    width: 120px;
    overflow: hidden;
    border-radius: 8px;
    border: 1px solid var(--border-subtle);
    background: var(--bg-hover);
  }

  /* Context blocks (action/branch-history tags rendered as tool-call-style cards) */
  .context-block-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-left: 2px;
  }

  .user-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .context-card {
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-primary);
  }

  .context-card-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 8px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .context-card-header:hover {
    background: var(--bg-hover);
  }

  .context-chevron {
    display: flex;
    align-items: center;
    width: 12px;
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .context-card-header :global(.context-icon) {
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .context-label {
    font-weight: 500;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) * 0.95);
  }

  .context-card-content {
    border-top: 1px solid var(--border-subtle);
    padding: 8px 10px;
    max-height: 200px;
    overflow-y: auto;
  }

  .context-card-content pre {
    margin: 0;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.9);
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.5;
  }

  .context-card-content::-webkit-scrollbar {
    width: 4px;
  }

  .context-card-content::-webkit-scrollbar-track {
    background: transparent;
  }

  .context-card-content::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 2px;
  }

  /* Assistant messages */
  .assistant-message {
    display: flex;
  }

  .assistant-content {
    position: relative;
    flex: 1;
    min-width: 0;
  }

  .assistant-content-has-copy {
    padding-right: 28px;
  }

  /* The bubble clips (overflow: hidden): the copy button's touch area moves
     in from the corner to stay whole. The assistant's starts at the button's
     top, short of the message above. */
  .human-bubble :global(.message-copy-action) {
    --touch-hit-dx: calc(0.375rem - 13px);
    --touch-hit-dy: calc(13px - 0.375rem);
  }

  .assistant-content :global(.message-copy-action) {
    --touch-hit-dy: 13px;
  }

  .human-bubble:focus-within :global(.message-copy-action),
  .assistant-content:focus-within :global(.message-copy-action),
  :global(.message-copy-action:focus-visible) {
    opacity: 1;
  }

  @media (hover: none), (pointer: coarse) {
    :global(.message-copy-action) {
      opacity: 1;
    }
  }

  /* ----- Tool calls ------------------------------------------------------ */

  .tool-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    min-width: 0;
  }

  /* Inline diagram from a successful render_pikchr call — rendered through the
     markdown pipeline, so it picks up the shared diagram styles (fullscreen
     zoom affordance included); only the message margins are tightened to sit
     inside the tool group. */
  .tool-diagram :global(.markdown-diagram) {
    margin: 4px 0;
  }

  .tool-caret {
    display: inline-block;
    flex-shrink: 0;
    width: 8px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    line-height: 1;
    transition: transform 0.15s ease;
  }

  .tool-caret-expanded {
    transform: rotate(90deg);
  }

  .tool-args-preview {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--size-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ACP metadata cards */
  .acp-event-row {
    display: flex;
  }

  .acp-event-card {
    width: 100%;
    min-width: 0;
    overflow: hidden;
  }

  .acp-event-header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
  }

  .acp-event-header:hover .acp-event-title {
    text-decoration: underline;
  }

  .acp-event-title {
    flex-shrink: 0;
    font-weight: 500;
  }

  .acp-event-body {
    margin-top: 4px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-primary);
    padding: 8px 10px;
    font-size: var(--size-xs);
  }

  .acp-event-body pre {
    margin: 0;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--text-muted);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.9);
    line-height: 1.5;
  }

  .config-list,
  .command-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .config-option {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(140px, 220px);
    gap: 8px;
    align-items: center;
    color: var(--text-muted);
  }

  .config-option select {
    min-width: 0;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    background: var(--bg-chrome);
    color: var(--text-muted);
    padding: 3px 6px;
    font: inherit;
  }

  .command-row,
  .slash-command-item {
    display: grid;
    grid-template-columns: minmax(96px, max-content) minmax(0, 1fr);
    gap: 8px;
    width: 100%;
    align-items: baseline;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    padding: 4px 6px;
    text-align: left;
    font: inherit;
    cursor: pointer;
  }

  .command-row:hover,
  .slash-command-item:hover {
    background: var(--bg-hover);
  }

  .command-row span,
  .slash-command-item span {
    color: var(--text-primary);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.95);
  }

  .command-row small,
  .slash-command-item small {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-faint);
    white-space: nowrap;
  }

  /* Thinking indicator */
  .thinking {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    padding: 4px 0;
  }

  /* Still running, but waiting on a background task rather than working —
     called out so the wait (and its Stop) reads as a distinct state. */
  .thinking-waiting {
    color: var(--ui-warning);
  }

  /* A named task under the wait, with a stop of its own — indented so the
     wait row above reads as the session-level state (and keeps the
     session-level Stop). */
  .hold-task-row {
    padding-left: 22px;
  }

  /* What a per-task stop answered, under the row it is about — the agent
     stopped nothing, or the request failed. On the row rather than in `error`,
     which would replace the whole transcript with an error card for what is
     usually just "that task had already finished". */
  .hold-task-notice {
    padding: 0 0 4px 22px;
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  /* With nothing to report the region stays mounted — that is what lets a
     later notice be announced — but out of flow, so it adds neither height nor
     one of the column's 12px gaps to a row that looks untouched. Hidden by
     clipping rather than `display`/`visibility`, which would drop it from the
     accessibility tree and undo the point. */
  .hold-task-notice-idle {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
  }

  .note-followup-row {
    display: flex;
    justify-content: center;
    padding: 4px 0;
  }

  /* ----- Input wrapper + queue popover ----------------------------------- */

  .auth-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .input-wrapper {
    order: -2;
    flex-shrink: 0;
  }

  /* In the 48px the keyboard leaves on a phone held sideways
     (dialog-content's `cramped`) the composer's row needs every pixel, so
     the drop-target border goes top and bottom. */
  :global([data-cramped]) .session-chat-pane {
    border-block-width: 0;
  }

  .queue-popover,
  .slash-command-popover {
    display: flex;
    flex-direction: column;
    gap: 0;
    margin: 0 14px;
    margin-left: calc(14px + var(--dialog-safe-left, 0px));
    margin-right: calc(14px + var(--dialog-safe-right, 0px));
    border: 1px solid var(--border-muted);
    border-bottom: none;
    border-radius: 10px 10px 0 0;
    background: var(--bg-elevated);
    overflow: hidden;
  }

  .queue-item {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 6px 10px;
    font-size: calc(var(--size-xs) * 0.95);
    line-height: 1.4;
    border-bottom: 1px solid var(--border-muted);
  }

  .queue-item:last-child {
    border-bottom: none;
  }

  .queue-item-label {
    flex-shrink: 0;
    font-size: calc(var(--size-xs) * 0.85);
    font-weight: 500;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .queue-item-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .queue-item-text {
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .queue-item-images {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .queue-item-image {
    position: relative;
    display: inline-block;
    width: 28px;
    height: 28px;
    overflow: hidden;
    border-radius: 6px;
    border: 1px solid var(--border-subtle);
    background: var(--bg-hover);
  }

  .queue-item-error {
    color: var(--ui-danger);
    white-space: normal;
    word-break: break-word;
  }

  /* ----- Pipeline stop area ---------------------------------------------- */

  /* Same chrome as .chat-composer: the error line (if any) above, the
     button right-aligned beneath it, and the same bottom inset, as it ends
     the dialog in the composer's place. */
  .pipeline-stop-area {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 14px;
    padding-bottom: calc(10px + var(--dialog-safe-bottom, 0px));
    padding-left: calc(14px + var(--dialog-safe-left, 0px));
    padding-right: calc(14px + var(--dialog-safe-right, 0px));
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    flex-shrink: 0;
  }

  .pipeline-stop-button {
    align-self: flex-end;
  }

  /* ======================================================================= */
  /* Markdown content (assistant messages)                                   */
  /* ======================================================================= */

  .markdown-content {
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.6;
  }

  .markdown-content :global(p) {
    margin: 0 0 0.5em 0;
  }

  .markdown-content :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3),
  .markdown-content :global(h4) {
    margin: 0.75em 0 0.5em 0;
    font-weight: 600;
    line-height: 1.3;
  }

  .markdown-content :global(h1:first-child),
  .markdown-content :global(h2:first-child),
  .markdown-content :global(h3:first-child),
  .markdown-content :global(h4:first-child) {
    margin-top: 0;
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
    margin: 0.5em 0;
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

  .markdown-content :global(a:hover) {
    text-decoration: underline;
  }

  .markdown-content :global(strong) {
    font-weight: 600;
  }

  .markdown-content :global(hr) {
    margin: 0.75em 0;
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

  /* The mobile layout (MOBILE_LAYOUT_QUERY, viewport.svelte.ts). */
  @media (max-width: 768px),
    (pointer: coarse) and (max-device-width: 500px),
    (pointer: coarse) and (max-device-height: 500px) {
    .modal-content {
      padding: 12px;
      padding-left: calc(12px + var(--dialog-safe-left, 0px));
      padding-right: calc(12px + var(--dialog-safe-right, 0px));
    }
  }
</style>
