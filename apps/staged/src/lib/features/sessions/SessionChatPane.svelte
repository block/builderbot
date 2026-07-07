<!--
  SessionChatPane.svelte — View an AI session (live or historical)

  The single session viewer used everywhere in the app. Shows the message
  transcript with user messages as right-aligned chat bubbles, assistant
  messages as plain markdown, and tool calls as collapsible cards with
  argument previews.

  Features:
  - Text input always available at the bottom
  - Send button when idle, Stop button when running
  - Message queue: hitting Enter while running persists a follow-up for backend drain
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
  import { onDestroy, tick, untrack } from 'svelte';
  import { slide } from 'svelte/transition';
  import X from '@lucide/svelte/icons/x';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import Info from '@lucide/svelte/icons/info';
  import CircleStop from '@lucide/svelte/icons/circle-stop';
  import Send from '@lucide/svelte/icons/send';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import Clock from '@lucide/svelte/icons/clock';
  import CircleAlert from '@lucide/svelte/icons/circle-alert';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import CircleSlash from '@lucide/svelte/icons/circle-slash';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Zap from '@lucide/svelte/icons/zap';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import FileText from '@lucide/svelte/icons/file-text';
  import Paperclip from '@lucide/svelte/icons/paperclip';
  import ImagePlus from '@lucide/svelte/icons/image-plus';
  import Plus from '@lucide/svelte/icons/plus';
  import Spinner from '../../shared/Spinner.svelte';
  import { isResumableReason } from '../../types';
  import type {
    AcpConfigSelection,
    AcpConfigValueSelection,
    Session,
    SessionMessage,
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
    getImageData,
    getSession,
    getSessionAcpMetadataMessages,
    getSessionMessages,
    getSessionMessagesSince,
    handleExternalLinkClick,
    deleteQueuedSessionMessage,
    listQueuedSessionMessages,
    queueSessionMessage,
    resumeSession,
    sendQueuedSessionMessage,
    type AcpConfigDiscovery,
    type AcpConfigSelector,
  } from '../../api/commands';
  import { listenToEvent, type UnlistenFn } from '../../transport';
  import AcpFixedConfigPicker from '../agents/AcpFixedConfigPicker.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { buildAcpConfigSelection } from '../agents/acpConfigSelection';
  import HashtagInput from './HashtagInput.svelte';
  import {
    buildBranchHashtagItems,
    findHashtagItemForReference,
    renderHashtagTokens as renderHashtagTokensShared,
  } from './hashtagItems';
  import * as Alert from '$lib/components/ui/alert';
  import { Button } from '$lib/components/ui/button';
  import { subscribeDragDrop } from '../branches/dragDrop';
  import {
    isImageFile,
    isMaybeTextFile,
    insertFilePathsAtCursor,
  } from '../branches/branchCardHelpers';
  import { hasXmlBlocks, sessionEndMessage } from './sessionModalHelpers';
  import {
    buildAcpTranscriptGroups,
    diffsFromAcpContent,
    displayLocations,
    formatJson,
    groupRichToolsByVerb,
    latestAvailableCommands,
    simpleUnifiedDiff,
    terminalRefsFromAcpContent,
    toolResultText,
    type AcpCommand,
    type AcpTranscriptEvent,
    type AcpTranscriptGroup,
    type RichToolItem,
  } from './acpTranscript';
  import {
    displayRootKey,
    normalizeDisplayRoots,
    resolveDisplayRoots,
    type DisplayRootInput,
  } from './pathDisplayRoots';
  import PipelineSteps from './PipelineSteps.svelte';
  import { highlightMatches, clearHighlights, scrollToMatch } from '../../shared/textHighlight';
  import '../../shared/markdown/diagramStyles.css';
  import { extractMarkdownDiagramFences } from '../../shared/markdown/diagramFormats';
  import DiagramViewerModal from '../../shared/markdown/DiagramViewerModal.svelte';
  import {
    getMarkdownDiagramSvgMarkup,
    isMarkdownDiagramActivationKey,
  } from '../../shared/markdown/diagramViewer';
  import { renderMarkdown as renderSharedMarkdown } from '../../shared/markdown/renderMarkdown';
  import { loadPikchrRenderer, type PikchrRenderer } from '../../shared/markdown/pikchrRendering';
  import { getNoteFollowupLabel, noteActivityLabel, type LinkedNoteContext } from './noteFreshness';
  import NoteActivityCard from './NoteActivityCard.svelte';
  import { splitAtNoteIndicator } from './noteIndicators';
  import type { HashtagClickInfo } from '../references/referenceHistory.svelte';
  import { latestAcpConfigDiscoveryFromMetadata } from './acpConfigMetadata';

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
    onHashtagClick?: (click: HashtagClickInfo) => void;
    onSessionChange?: (session: Session | null) => void;
    onSearchStateChange?: (state: { matchCount: number; currentIndex: number }) => void;
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
    onHashtagClick,
    onSessionChange,
    onSearchStateChange,
    compact = false,
  }: Props = $props();

  // =========================================================================
  // State
  // =========================================================================

  let session = $state<Session | null>(null);
  let messages = $state<SessionMessage[]>([]);
  let acpMetadataMessages = $state<SessionMessage[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let cancelling = $state(false);
  let messagesEl: HTMLDivElement;
  let inputEl: HTMLElement | null = $state(null);
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollInFlight = false;
  let unlistenStatus: UnlistenFn | null = null;
  let statusEventVersion = 0;
  let closed = false;

  let inputText = $state('');
  let queuedMessages = $state<QueuedSessionMessage[]>([]);
  let queueActionIds = $state<Set<string>>(new Set());
  let copiedId = $state<number | string | null>(null);
  let expandedTools = $state<Set<string>>(new Set());
  let displayRoots = $state<string[]>([]);
  let currentDisplayRootKey = '';
  let discoveredFollowupConfig = $state<AcpConfigDiscovery | null>(null);
  let followupConfigLoading = $state(false);
  let followupConfigError = $state<string | null>(null);
  let selectedFollowupModelValue = $state<string | null>(null);
  let selectedFollowupEffortValue = $state<string | null>(null);
  let followupModelTouched = $state(false);
  let followupEffortTouched = $state(false);
  let followupModelStateKey = $state<string | null>(null);
  let followupEffortStateKey = $state<string | null>(null);
  let followupDiscoveryRun = 0;
  const footerControlClass =
    'h-9 gap-1.5 rounded-md border border-[var(--border-muted)] bg-[var(--bg-primary)] px-4 py-2 text-sm font-medium text-muted-foreground shadow-none transition-colors hover:bg-[var(--bg-hover)] hover:text-foreground max-[640px]:h-11 max-[640px]:justify-center';

  let isLive = $derived(session?.status === 'running');
  let hasQueuedMessages = $derived(queuedMessages.length > 0);
  let noteFollowupLabel = $derived(getNoteFollowupLabel(session, messages, noteInfo));
  let assistantMarkdownContent = $derived(
    messages
      .filter((message) => message.role === 'assistant')
      .map((message) => message.content)
      .join('\n\n')
  );
  let sessionHasPikchr = $derived(
    extractMarkdownDiagramFences(assistantMarkdownContent).some(
      (diagram) => diagram.language === 'pikchr'
    )
  );
  let pikchrRenderer = $state<PikchrRenderer | null>(null);
  let pikchrRendererLoadKey = $derived(`${sessionId}\0${assistantMarkdownContent}`);
  let pikchrRendererLoadFailedKey = $state<string | null>(null);
  let pikchrRendererLoadFailed = $derived(pikchrRendererLoadFailedKey === pikchrRendererLoadKey);
  let diagramViewerSvg = $state<string | null>(null);
  let metadataFollowupConfig = $derived.by(() =>
    latestAcpConfigDiscoveryFromMetadata(session?.provider, acpMetadataMessages)
  );
  let rediscoveredFollowupConfig = $derived(
    discoveredFollowupConfig?.providerId === session?.provider ? discoveredFollowupConfig : null
  );
  let followupModelSelector = $derived(
    metadataFollowupConfig?.model ?? rediscoveredFollowupConfig?.model ?? null
  );
  let followupEffortSelector = $derived(
    metadataFollowupConfig?.effort ?? rediscoveredFollowupConfig?.effort ?? null
  );
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
          session?.acpConfigSelection?.effort ?? null,
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

  /** Whether the initial load has rendered — transitions are suppressed until then. */
  let animateNewMessages = false;

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
  let imagePreviews = $state<Map<string, string>>(new Map());
  let imageFileInput = $state<HTMLInputElement>();

  // Drag-and-drop state
  let dragOver = $state(false);
  let modalElement: HTMLElement | null = $state(null);

  // Shared image data cache for both reply previews and message history images.
  // Maps image ID → data URL. Loaded lazily when needed.
  let messageImageCache = $state<Map<string, string>>(new Map());

  // Load previews for attached images
  $effect(() => {
    for (const id of replyImageIds) {
      if (!imagePreviews.has(id)) {
        getImageData(id)
          .then((dataUrl) => {
            imagePreviews = new Map(imagePreviews);
            imagePreviews.set(id, dataUrl);
          })
          .catch(() => {
            // Image may have been deleted — insert sentinel to prevent infinite retry
            imagePreviews = new Map(imagePreviews);
            imagePreviews.set(id, '');
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
        getImageData(id)
          .then((dataUrl) => {
            messageImageCache = new Map(messageImageCache);
            messageImageCache.set(id, dataUrl);
          })
          .catch(() => {
            messageImageCache = new Map(messageImageCache);
            messageImageCache.set(id, '');
          });
      }
    }
  });

  function openImagePicker() {
    imageFileInput?.click();
  }

  async function handleImageFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;
    for (const file of Array.from(input.files)) {
      await addImageFile(file);
    }
    input.value = '';
  }

  async function addImageFile(file: File) {
    if (!projectId) return;
    const buffer = await file.arrayBuffer();
    const bytes = new Uint8Array(buffer);
    const chunks: string[] = [];
    for (let i = 0; i < bytes.length; i += 8192) {
      chunks.push(String.fromCharCode(...bytes.subarray(i, i + 8192)));
    }
    const base64 = btoa(chunks.join(''));
    try {
      const image = await createImageFromData(
        branchId ?? null,
        projectId,
        file.name,
        file.type,
        base64,
        true
      );
      replyImageIds = [...replyImageIds, image.id];
      const dataUrl = `data:${file.type};base64,${base64}`;
      imagePreviews = new Map(imagePreviews);
      imagePreviews.set(image.id, dataUrl);
    } catch (err) {
      console.error('Failed to attach image:', err);
    }
  }

  function handleImagePaste(e: ClipboardEvent) {
    if (!active || !canAttachImages || isLive) return;
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const item of Array.from(items)) {
      if (item.type.startsWith('image/')) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) void addImageFile(file);
      }
    }
  }

  function removeReplyImage(imageId: string) {
    replyImageIds = replyImageIds.filter((id) => id !== imageId);
    imagePreviews = new Map(imagePreviews);
    imagePreviews.delete(imageId);
    deleteImage(imageId).catch((err) => {
      console.error('Failed to delete image:', err);
    });
  }

  // Drag-and-drop files (via Tauri native drag-drop events)
  async function handleFileDrop(paths: string[]) {
    if (!projectId) return;
    const imagePaths = paths.filter((p) => isImageFile(p));
    const textPaths = paths.filter((p) => isMaybeTextFile(p));
    const bid = branchId ?? null;
    const pid = projectId;
    const newIds: string[] = [];
    for (const path of imagePaths) {
      try {
        const image = await createImage(bid, pid, path, true);
        newIds.push(image.id);
      } catch (e) {
        console.error('Failed to create image from dropped file:', e);
      }
    }
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
    stopPolling();
    unlistenStatus?.();
  });

  // This pane can be mounted once and reused across opens (the `active` prop toggles
  // visibility), so the session must be (re)loaded reactively rather than in
  // onMount — otherwise it would load once with whatever sessionId was set at
  // mount time (often empty) and never refresh. Re-run whenever the modal is
  // opened or the target session changes.
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
    loadSession().then(() => {
      if (closed || !active || sessionId !== id) return;
      if (session?.status === 'running') {
        startPolling();
      }
      // Focus input on open
      tick().then(() => inputEl?.focus());
    });
  });

  $effect(() => {
    onSessionChange?.(session);
  });

  $effect(() => {
    const isActive = active;
    const id = sessionId;
    if (!isActive || !id) return;

    const unlisten = listenToEvent<SessionStatusPayload>('session-status-changed', (payload) => {
      if (payload.sessionId !== id) return;
      statusEventVersion += 1;
      session =
        session?.id === id
          ? {
              ...session,
              status: payload.status,
              errorMessage: payload.errorMessage ?? session.errorMessage,
              completionReason: payload.completionReason ?? session.completionReason,
              updatedAt: Date.now(),
            }
          : session;
      refreshQueuedMessages();
      if (payload.status === 'running') {
        startPolling();
      } else {
        stopPolling();
      }
    });
    unlistenStatus = unlisten;

    return () => {
      unlistenStatus?.();
      unlistenStatus = null;
    };
  });

  function isComposerFocused(): boolean {
    return document.activeElement === inputEl;
  }

  // =========================================================================
  // Data loading
  // =========================================================================

  /** Initial full load. */
  async function loadSession() {
    if (closed) return;
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
      acpMetadataMessages = [];
      queuedMessages = [];
    }
    loading = true;
    error = null;
    try {
      const [s, msgsResult, acpMsgs, queued] = await Promise.all([
        getSession(sessionId),
        getSessionMessages(sessionId),
        getSessionAcpMetadataMessages(sessionId),
        listQueuedSessionMessages(sessionId),
      ]);
      if (closed) return;
      if (!s) {
        error = 'Session not found';
        return;
      }
      session = s;
      messages = msgsResult.data;
      queuedMessages = queued;
      if (msgsResult.revalidating) {
        msgsResult.revalidating
          .then((fresh) => {
            if (closed) return;
            messages = fresh;
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
      animateNewMessages = true;
      scrollToBottom();
    }
  }

  /** Incremental poll — re-fetch last message (may have grown) + any new ones. */
  async function poll() {
    if (!session || closed || pollInFlight) return;
    pollInFlight = true;
    try {
      // Fetch session status
      const statusVersionBeforeFetch = statusEventVersion;
      const s = await getSession(sessionId);
      if (closed) return;
      const statusEventDuringFetch = statusEventVersion !== statusVersionBeforeFetch;
      if (s && !statusEventDuringFetch) session = s;
      await refreshQueuedMessages();

      // Incremental message fetch
      if (messages.length === 0) {
        const [{ data: msgs }, acpMsgs] = await Promise.all([
          getSessionMessages(sessionId),
          getSessionAcpMetadataMessages(sessionId),
        ]);
        if (closed) return;
        acpMetadataMessages = acpMsgs;
        if (msgs.length > 0) {
          messages = msgs;
          scrollToBottomIfNear(true);
        }
      } else {
        const lastId = messages[messages.length - 1].id;
        const [updated, acpMsgs] = await Promise.all([
          getSessionMessagesSince(sessionId, lastId),
          getSessionAcpMetadataMessages(sessionId),
        ]);
        if (closed) return;
        acpMetadataMessages = acpMsgs;
        if (updated.length > 0) {
          const prev = messages.slice(0, -1);
          messages = [...prev, ...updated];
          if (updated.length > 1 || updated[0].id !== lastId) {
            scrollToBottomIfNear(true);
          }
        }
      }

      // Stop polling when session is done. Backend-owned queue drain may emit
      // a fresh running event while this poll is in flight; that event is
      // fresher than this status fetch and keeps polling alive.
      if (s && s.status !== 'running' && !statusEventDuringFetch && session?.status !== 'running') {
        stopPolling();
      }
    } catch {
      // Polling errors are expected during shutdown — silently ignore
    } finally {
      pollInFlight = false;
    }
  }

  async function refreshQueuedMessages() {
    if (!sessionId || closed) return;
    try {
      queuedMessages = await listQueuedSessionMessages(sessionId);
    } catch (e) {
      console.error('Failed to refresh queued messages:', e);
    }
  }

  function startPolling() {
    if (pollTimer) return;
    pollTimer = setInterval(poll, 500);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    pollInFlight = false;
  }

  // =========================================================================
  // Message sending & queue
  // =========================================================================

  let sending = $state(false);
  let noteFollowupSending = $state(false);

  async function handleSend() {
    const text = inputText.trim();
    if (!text) return;
    inputText = '';
    const imageIdsToSend = replyImageIds.length > 0 ? [...replyImageIds] : undefined;
    replyImageIds = [];
    imagePreviews = new Map();
    // Reset textarea height after clearing (oninput won't fire for programmatic changes)
    tick().then(() => autoResize());

    if (isLive) {
      try {
        await queueSessionMessage(sessionId, text, imageIdsToSend, branchId ?? null);
        await refreshQueuedMessages();
      } catch (e) {
        error = `Failed to queue: ${e instanceof Error ? e.message : String(e)}`;
        inputText = text;
        replyImageIds = imageIdsToSend ?? [];
      }
      return;
    }

    // Session is idle — send immediately
    await sendMessage(text, imageIdsToSend);
  }

  /** Actually send a message to the backend and start the agent. */
  async function sendMessage(
    text: string,
    imageIds?: string[],
    sendingLocked = false,
    target: SendMessageTarget | null = null
  ) {
    const targetSessionId = target?.sessionId ?? session?.id;
    const targetBranchId = target ? target.branchId : (branchId ?? null);
    if (!targetSessionId || session?.id !== targetSessionId || (!sendingLocked && sending)) return;
    if (!sendingLocked) sending = true;
    error = null;
    const acpConfigSelection: AcpConfigSelection | undefined =
      followupAcpConfigSelection ?? undefined;
    try {
      await resumeSession(targetSessionId, text, imageIds, targetBranchId, acpConfigSelection);
      if (session?.id !== targetSessionId) return;
      // Backend sets status to running and emits an event.
      // Force an immediate poll to pick up the new user message + status.
      session = {
        ...session,
        status: 'running',
        acpConfigSelection: acpConfigSelection ?? session.acpConfigSelection ?? null,
      };
      startPolling();
      scrollToBottom();
    } catch (e) {
      error = `Failed to send: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      if (!sendingLocked) sending = false;
    }
  }

  async function handleNoteFollowupClick() {
    if (!session || !noteInfo || sending || noteFollowupSending) return;
    const target: SendMessageTarget = { sessionId: session.id, branchId: branchId ?? null };
    const hasParsedNote = noteInfo.hasParsedNote;
    sending = true;
    noteFollowupSending = true;
    error = null;
    try {
      const prompt = await buildNoteFollowupMessage(
        target.sessionId,
        target.branchId,
        hasParsedNote
      );
      if (session?.id !== target.sessionId || (branchId ?? null) !== target.branchId) return;
      await sendMessage(prompt, undefined, true, target);
    } catch (e) {
      error = `Failed to send: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      noteFollowupSending = false;
      sending = false;
    }
  }

  async function handleDeleteQueuedMessage(id: string) {
    if (queueActionIds.has(id)) return;
    queueActionIds = new Set(queueActionIds).add(id);
    try {
      await deleteQueuedSessionMessage(id);
      await refreshQueuedMessages();
    } catch (e) {
      error = `Failed to remove queued message: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      const next = new Set(queueActionIds);
      next.delete(id);
      queueActionIds = next;
    }
  }

  async function handleSendQueuedMessage(id: string) {
    if (isLive || sending || queueActionIds.has(id)) return;
    queueActionIds = new Set(queueActionIds).add(id);
    error = null;
    try {
      await sendQueuedSessionMessage(id);
      await refreshQueuedMessages();
      if (session) session = { ...session, status: 'running' };
      startPolling();
      scrollToBottom();
    } catch (e) {
      error = `Failed to send queued message: ${e instanceof Error ? e.message : String(e)}`;
      await refreshQueuedMessages();
    } finally {
      const next = new Set(queueActionIds);
      next.delete(id);
      queueActionIds = next;
    }
  }

  async function handleCancel() {
    if (!session || cancelling) return;
    cancelling = true;
    try {
      await cancelSession(session.id);
    } catch (e) {
      error = `Failed to cancel: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      cancelling = false;
    }
  }

  // =========================================================================
  // Input handling
  // =========================================================================

  function handleInputKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
      e.preventDefault();
      handleSend();
    }
  }

  function autoResize() {
    if (!inputEl) return;
    inputEl.style.height = 'auto';
    inputEl.style.overflow = 'hidden';
    const borderY = inputEl.offsetHeight - inputEl.clientHeight;
    const maxHeight = 120;
    const height = Math.min(inputEl.scrollHeight + borderY, maxHeight);
    inputEl.style.height = height + 'px';
    if (height >= maxHeight) {
      inputEl.style.overflow = 'auto';
    }
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

  function handleScroll() {
    if (!messagesEl) return;
    const { scrollTop, scrollHeight, clientHeight } = messagesEl;
    const atBottom = scrollHeight - scrollTop - clientHeight < 1;

    if (scrollTop < lastScrollTop && !atBottom) {
      // Scroll position moved upward — user scrolled up (any input method)
      userScrolledUp = true;
    }
    if (atBottom) {
      // They're back at bottom — re-enable auto-scroll
      userScrolledUp = false;
    }
    lastScrollTop = scrollTop;
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

  function renderMarkdown(content: string): string {
    return renderSharedMarkdown(content, {
      pikchrRenderer,
      renderInlineText: hashtagItems.length
        ? (text) => renderHashtagTokens(text, hashtagItems)
        : undefined,
    });
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
    if (openDiagramViewerFromEvent(event)) return;

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
    if (isMarkdownDiagramActivationKey(event) && openDiagramViewerFromEvent(event)) return;
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

    const tagPattern = /<(action|branch-history|launch-context)>([\s\S]*?)<\/\1>/g;
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
            : 'Launch context';
      const icon = tag === 'action' ? Zap : GitBranch;
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
      scrollToMatch(matchElements[0]);
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
    scrollToMatch(matchElements[currentMatchIndex]);
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
    scrollToMatch(matchElements[currentMatchIndex]);
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
      selectedFollowupModelValue = initialSelectorValue(
        followupModelSelector,
        session?.acpConfigSelection?.model ?? null
      );
      followupModelTouched = false;
      followupModelStateKey = key;
    }
  });

  $effect(() => {
    const key = selectorStateKey(
      session?.id,
      followupEffortSelector,
      session?.acpConfigSelection?.effort ?? null
    );
    if (key !== followupEffortStateKey) {
      selectedFollowupEffortValue = initialSelectorValue(
        followupEffortSelector,
        session?.acpConfigSelection?.effort ?? null
      );
      followupEffortTouched = false;
      followupEffortStateKey = key;
    }
  });

  export function close() {
    if (closed) return;
    closed = true;
    stopPolling();
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
    if (!selector || selector.options.length === 0) return null;
    if (
      storedSelection?.configId === selector.configId &&
      selector.options.some((option) => option.valueId === storedSelection.valueId)
    ) {
      return storedSelection.valueId;
    }
    if (selector.options.some((option) => option.valueId === selector.currentValueId)) {
      return selector.currentValueId;
    }
    return selector.options[0]?.valueId ?? null;
  }

  function shouldSendFollowupSelectorSelection(
    selector: AcpConfigSelector | null,
    valueId: string | null,
    storedSelection: AcpConfigValueSelection | null,
    touched: boolean
  ): boolean {
    if (!selector || !valueId) return false;
    if (touched) return true;
    return storedSelection?.configId === selector.configId && storedSelection.valueId === valueId;
  }

  function handleFollowupModelChange(value: string) {
    selectedFollowupModelValue = value;
    followupModelTouched = true;
  }

  function handleFollowupEffortChange(value: string) {
    selectedFollowupEffortValue = value;
    followupEffortTouched = true;
  }

  let grouped = $derived.by(() =>
    buildAcpTranscriptGroups(messages, acpMetadataMessages, displayRoots)
  );
  let slashCommands = $derived.by(() => latestAvailableCommands(acpMetadataMessages));
  let slashQuery = $derived.by(() => {
    const trimmed = inputText.trimStart();
    return trimmed.startsWith('/') ? trimmed.slice(1).toLowerCase() : null;
  });
  let matchingSlashCommands = $derived.by(() => {
    if (slashQuery === null) return [];
    return slashCommands
      .filter((command) => command.name.toLowerCase().includes(slashQuery))
      .slice(0, 6);
  });

  let noteBearingMessageIds = $derived.by(() => {
    return messages
      .filter(
        (message) => message.role === 'assistant' && splitAtNoteIndicator(message.content).hasNote
      )
      .map((message) => message.id);
  });

  let isPipelinePrelude = $derived(isLive && !!session?.pipeline && grouped.length === 0);

  function insertSlashCommand(command: AcpCommand) {
    inputText = `/${command.name}${command.inputHint ? ' ' : ''}`;
    tick().then(() => {
      inputEl?.focus();
      autoResize();
    });
  }

  function acpEventSummary(event: AcpTranscriptEvent): string {
    if (event.kind === 'plan_update') {
      const entries = arrayProp(event.content, 'entries');
      return entries.length === 1 ? '1 step' : `${entries.length} steps`;
    }
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

  function planEntries(content: unknown): Record<string, unknown>[] {
    return arrayProp(content, 'entries');
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

  function groupKey(group: AcpTranscriptGroup): string {
    if (group.type === 'tools') return `t-${group.items[0].key}`;
    if (group.type === 'acp') return `a-${group.event.id}`;
    return `m-${group.message.id}`;
  }

  /** Slide-in transition that is suppressed during the initial load. */
  function messageSlide(node: Element) {
    if (!animateNewMessages) return { duration: 0 };
    return slide(node, { duration: SLIDE_DURATION });
  }
</script>

<svelte:window onpaste={handleImagePaste} />

{#snippet toolStatusIcon(statusTone: RichToolItem['statusTone'])}
  {#if statusTone === 'running'}
    <Clock size={11} />
  {:else if statusTone === 'success'}
    <CircleCheck size={11} />
  {:else if statusTone === 'danger'}
    <CircleAlert size={11} />
  {:else if statusTone === 'cancelled'}
    <CircleSlash size={11} />
  {:else}
    <CircleDot size={11} />
  {/if}
{/snippet}

{#snippet richToolCard(item: RichToolItem, nested: boolean)}
  {@const isExpanded = expandedTools.has(item.key)}
  {@const resultText = toolResultText(item)}
  {@const rawInputText = formatJson(item.rawInput)}
  {@const rawOutputText = formatJson(item.rawOutput)}
  {@const diffs = diffsFromAcpContent(item.content, displayRoots)}
  {@const locations = displayLocations(item.locations, displayRoots)}
  {@const terminalRefs = terminalRefsFromAcpContent(item.content)}
  {@const hasDetails =
    !!resultText ||
    !!rawInputText ||
    !!rawOutputText ||
    diffs.length > 0 ||
    locations.length > 0 ||
    terminalRefs.length > 0}
  <div class="tool-card" class:tool-card-nested={nested}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="tool-header"
      class:tool-header-expandable={hasDetails}
      onclick={() => hasDetails && toggleTool(item.key)}
    >
      <span
        class="tool-caret"
        class:tool-caret-expanded={isExpanded}
        class:tool-caret-hidden={!hasDetails}>›</span
      >
      <span
        class="tool-status-dot"
        class:status-running={item.statusTone === 'running'}
        class:status-success={item.statusTone === 'success'}
        class:status-danger={item.statusTone === 'danger'}
        class:status-cancelled={item.statusTone === 'cancelled'}
      >
        {@render toolStatusIcon(item.statusTone)}
      </span>
      <span class="tool-name">{item.verb}</span>
      {#if item.detail}
        <span class="tool-args-preview">{item.detail}</span>
      {/if}
    </div>
    {#if isExpanded && hasDetails}
      <div class="tool-code-block" transition:slide={{ duration: SLIDE_DURATION }}>
        {#if (item.verb === 'Ran' || item.verb === 'Running') && item.detail}
          <div class="tool-code-command">$ {item.detail}</div>
        {/if}
        {#if locations.length > 0}
          <div class="tool-meta-row">
            {#each locations as location}
              <span class="tool-chip">{location}</span>
            {/each}
          </div>
        {/if}
        {#if rawInputText}
          <div class="tool-panel-label">Input</div>
          <pre class="tool-code-output">{rawInputText}</pre>
        {/if}
        {#each diffs as diff}
          <div class="tool-panel-label">{diff.path}</div>
          <pre class="tool-code-output diff-output">{simpleUnifiedDiff(diff)}</pre>
        {/each}
        {#if terminalRefs.length > 0}
          <div class="tool-panel-label">Terminal</div>
          <div class="tool-meta-row">
            {#each terminalRefs as terminalRef}
              <span class="tool-chip">{terminalRef}</span>
            {/each}
          </div>
        {/if}
        {#if resultText}
          <div class="tool-panel-label">Output</div>
          <pre class="tool-code-output">{resultText}</pre>
        {/if}
        {#if rawOutputText && rawOutputText !== resultText}
          <div class="tool-panel-label">Raw output</div>
          <pre class="tool-code-output">{rawOutputText}</pre>
        {/if}
        <div
          class="tool-code-status"
          class:status-danger={item.statusTone === 'danger'}
          class:status-cancelled={item.statusTone === 'cancelled'}
        >
          {#if item.statusTone === 'success'}
            <Check size={11} />
          {/if}
          {item.statusLabel}
        </div>
      </div>
    {/if}
  </div>
{/snippet}

<div
  bind:this={modalElement}
  class={`session-chat-pane ${compact ? 'compact' : ''} ${dragOver ? 'drag-over' : ''}`}
>
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
      <PipelineSteps {sessionId} pipeline={session.pipeline} />
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
        {#each grouped as group, groupIdx (groupKey(group))}
          <div in:messageSlide class={group.type === 'user' ? 'user-group' : ''}>
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
                      <span class="human-text"
                        >{@html renderHashtagTokens(userText.trim(), hashtagItems)}</span
                      >
                    {/if}
                    {#if group.message.imageIds && group.message.imageIds.length > 0}
                      <div class="message-images">
                        {#each group.message.imageIds as imgId}
                          {#if messageImageCache.get(imgId)}
                            <img
                              class="message-image-thumb"
                              src={messageImageCache.get(imgId)}
                              alt="attachment"
                            />
                          {:else}
                            <div class="message-image-placeholder">
                              <ImagePlus size={16} />
                            </div>
                          {/if}
                        {/each}
                      </div>
                    {/if}
                    <Button
                      variant="ghost"
                      size="icon"
                      class="message-copy-action absolute top-1.5 right-1.5 size-auto rounded p-[3px] text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-hover)] hover:text-foreground group-hover/bubble:opacity-100 [&_svg]:!size-3"
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
                streaming: isLive && groupIdx === grouped.length - 1,
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
                        isLastGroup: groupIdx === grouped.length - 1,
                        isFirstNoteMessage: group.message.id === firstNoteMessageId,
                      })}
                    />
                  {/if}
                  {#if showAssistantCopy}
                    <Button
                      variant="ghost"
                      size="icon"
                      class="message-copy-action absolute top-0 right-0 size-auto rounded p-[3px] text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-hover)] hover:text-foreground group-hover/assistant:opacity-100 [&_svg]:!size-3"
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
                    {@render richToolCard(verbGroup.items[0], false)}
                  {:else}
                    {@const isGroupExpanded = expandedTools.has(verbGroup.key)}
                    <div class="tool-card">
                      <!-- svelte-ignore a11y_click_events_have_key_events -->
                      <!-- svelte-ignore a11y_no_static_element_interactions -->
                      <div
                        class="tool-header tool-header-expandable"
                        onclick={() => toggleTool(verbGroup.key)}
                      >
                        <span class="tool-caret" class:tool-caret-expanded={isGroupExpanded}>›</span
                        >
                        <span
                          class="tool-status-dot"
                          class:status-running={verbGroup.statusTone === 'running'}
                          class:status-success={verbGroup.statusTone === 'success'}
                          class:status-danger={verbGroup.statusTone === 'danger'}
                          class:status-cancelled={verbGroup.statusTone === 'cancelled'}
                        >
                          {@render toolStatusIcon(verbGroup.statusTone)}
                        </span>
                        <span class="tool-name">{verbGroup.verb}</span>
                        <span class="tool-args-preview">{verbGroup.summary}</span>
                      </div>
                    </div>
                    {#if isGroupExpanded}
                      <div transition:slide={{ duration: SLIDE_DURATION }}>
                        {#each verbGroup.items as item (item.key)}
                          {@render richToolCard(item, true)}
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
                      {#if event.kind === 'plan_update'}
                        <div class="plan-list">
                          {#each planEntries(event.content) as entry}
                            <div class="plan-entry">
                              <span class="plan-status">{stringProp(entry, 'status')}</span>
                              <span>{stringProp(entry, 'content')}</span>
                            </div>
                          {/each}
                        </div>
                      {:else if event.kind === 'config_options_update'}
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
          <div class="thinking" in:messageSlide>
            <Spinner size={14} />
            <span>Thinking…</span>
          </div>
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

    {#if session?.status === 'error' && session.errorMessage}
      <Alert.Root variant="destructive" class="mt-3">
        <AlertCircle />
        <Alert.Description>{session.errorMessage}</Alert.Description>
      </Alert.Root>
    {:else if session && session.status !== 'running' && session.status !== 'queued'}
      {#if isResumableReason(session.completionReason)}
        {@const isWarning =
          session.completionReason === 'crashed' || session.completionReason === 'app_quit'}
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
              Resume
            </Button>
          </Alert.Action>
        </Alert.Root>
      {/if}
    {/if}
  </div>

  <!-- Input area with queued messages and image previews -->
  <div class="input-wrapper">
    {#if isPipelinePrelude}
      <div class="input-area pipeline-stop-area">
        <span class="inline-flex" title="Stop workflow">
          <Button
            variant="destructive"
            size="icon"
            class="size-9 shrink-0 rounded-[10px] [&_svg]:!size-4"
            aria-label="Stop workflow"
            onclick={handleCancel}
            disabled={cancelling}
          >
            {#if cancelling}
              <Spinner size={16} />
            {:else}
              <CircleStop size={16} />
            {/if}
          </Button>
        </span>
      </div>
    {:else}
      {#if hasQueuedMessages}
        <div class="queue-popover">
          {#each queuedMessages as msg (msg.id)}
            <div class="queue-item">
              <span class="queue-item-label">{msg.status === 'sending' ? 'Sending' : 'Queued'}</span
              >
              <div class="queue-item-body">
                <span class="queue-item-text">{msg.content}</span>
                {#if msg.imageIds.length > 0}
                  <div class="queue-item-images">
                    {#each msg.imageIds as imageId}
                      {#if messageImageCache.get(imageId)}
                        <img src={messageImageCache.get(imageId)} alt="queued attachment" />
                      {:else}
                        <span class="queue-item-image-placeholder"><ImagePlus size={12} /></span>
                      {/if}
                    {/each}
                  </div>
                {/if}
                {#if msg.lastError}
                  <span class="queue-item-error">{msg.lastError}</span>
                {/if}
              </div>
              {#if !isLive}
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-[22px] shrink-0 rounded text-[var(--text-faint)] hover:bg-[var(--bg-hover)] hover:text-foreground disabled:opacity-40 [&_svg]:!size-3"
                  title="Send queued message"
                  aria-label="Send queued message"
                  disabled={msg.status !== 'queued' || queueActionIds.has(msg.id)}
                  onclick={() => handleSendQueuedMessage(msg.id)}
                >
                  {#if queueActionIds.has(msg.id)}
                    <Spinner size={12} />
                  {:else}
                    <Send size={12} />
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
      {#if canAttachImages && replyImageIds.length > 0}
        <div class="reply-images">
          {#each replyImageIds as imageId}
            <div class="group/thumb reply-image-thumb">
              {#if imagePreviews.get(imageId)}
                <img src={imagePreviews.get(imageId)} alt="attached" />
              {:else}
                <div class="reply-image-placeholder"><ImagePlus size={16} /></div>
              {/if}
              {#if !isLive}
                <Button
                  variant="ghost"
                  size="icon"
                  class="reply-image-remove-action absolute top-0.5 right-0.5 size-4 rounded-full bg-[var(--bg-deepest)] text-muted-foreground opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-chrome)] hover:text-foreground group-hover/thumb:opacity-100 [&_svg]:!size-2.5"
                  title="Remove image"
                  aria-label="Remove image"
                  onclick={() => removeReplyImage(imageId)}
                >
                  <X size={10} />
                </Button>
              {/if}
            </div>
          {/each}
          {#if !isLive}
            <Button
              variant="outline"
              size="icon"
              class="size-12 shrink-0 rounded-md border border-dashed border-[var(--border-muted)] bg-transparent text-[var(--text-faint)] shadow-none hover:border-[var(--border-emphasis)] hover:bg-transparent hover:text-muted-foreground [&_svg]:!size-4"
              title="Add image"
              aria-label="Add image"
              onclick={openImagePicker}
            >
              <Plus size={16} />
            </Button>
          {/if}
        </div>
      {/if}
      <div class="input-area">
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
          triggerClass={footerControlClass}
          onModelChange={handleFollowupModelChange}
          onEffortChange={handleFollowupEffortChange}
        />
        {#if canAttachImages}
          <input
            bind:this={imageFileInput}
            type="file"
            accept="image/png,image/jpeg,image/gif,image/webp,image/svg+xml"
            multiple
            class="file-input-hidden"
            onchange={handleImageFileSelect}
          />
          {#if replyImageIds.length === 0}
            <span class="inline-flex" title="Attach image">
              <Button
                variant="ghost"
                size="icon"
                class="size-9 shrink-0 rounded-[10px] text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
                aria-label="Attach image"
                onclick={openImagePicker}
                disabled={isLive}
              >
                <Paperclip size={16} />
              </Button>
            </span>
          {/if}
        {/if}
        <HashtagInput
          bind:textareaEl={inputEl}
          bind:value={inputText}
          class="message-input"
          placeholder={isLive ? 'Type to queue a follow-up…' : 'Send a message…'}
          rows={1}
          onkeydown={handleInputKeydown}
          oninput={autoResize}
          items={hashtagItems}
        />
        {#if isLive}
          <span class="inline-flex" title="Stop session">
            <Button
              variant="destructive"
              size="icon"
              class="size-9 shrink-0 rounded-[10px] [&_svg]:!size-4"
              aria-label="Stop session"
              onclick={handleCancel}
              disabled={cancelling}
            >
              {#if cancelling}
                <Spinner size={16} />
              {:else}
                <CircleStop size={16} />
              {/if}
            </Button>
          </span>
        {:else}
          <span class="inline-flex" title="Send message">
            <Button
              variant="outline"
              size="icon"
              class="size-9 shrink-0 rounded-[10px] shadow-none disabled:opacity-30 [&_svg]:!size-4"
              aria-label="Send message"
              onclick={handleSend}
              disabled={sending || !inputText.trim()}
            >
              {#if sending}
                <Spinner size={16} />
              {:else}
                <Send size={16} />
              {/if}
            </Button>
          </span>
        {/if}
      </div>
    {/if}
  </div>
</div>

<DiagramViewerModal
  open={diagramViewerSvg !== null}
  svgMarkup={diagramViewerSvg}
  onClose={() => (diagramViewerSvg = null)}
/>

<style>
  /* ======================================================================= */
  /* Modal shell                                                             */
  /* ======================================================================= */

  .session-chat-pane {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow: hidden;
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
  }

  .session-chat-pane.compact .messages {
    gap: 10px;
  }

  .session-chat-pane.compact .human-bubble {
    max-width: 92%;
  }

  .session-chat-pane.compact .input-area {
    padding: 8px 10px;
  }

  .session-chat-pane.compact .queue-popover,
  .session-chat-pane.compact .slash-command-popover {
    margin: 0 10px;
  }

  /* ----- Content area (scrollable) --------------------------------------- */

  .modal-content {
    flex: 1;
    overflow-x: hidden;
    overflow-y: auto;
    /* Keep scroll momentum inside the transcript so it doesn't chain to the
       page behind the keyboard-shrunk full-screen dialog. */
    overscroll-behavior: contain;
    padding: 16px;
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

  /* Images attached to user messages */
  .message-images {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 6px;
  }

  .message-image-thumb {
    width: 120px;
    max-height: 120px;
    object-fit: cover;
    border-radius: 8px;
    border: 1px solid var(--border-subtle);
    cursor: pointer;
  }

  .message-image-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    border-radius: 8px;
    border: 1px solid var(--border-subtle);
    background: var(--bg-hover);
    color: var(--text-faint);
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

  .human-bubble:focus-within :global(.message-copy-action),
  .assistant-content:focus-within :global(.message-copy-action),
  .reply-image-thumb:focus-within :global(.reply-image-remove-action),
  :global(.message-copy-action:focus-visible),
  :global(.reply-image-remove-action:focus-visible) {
    opacity: 1;
  }

  @media (hover: none), (pointer: coarse) {
    :global(.message-copy-action),
    :global(.reply-image-remove-action) {
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

  .tool-card {
    overflow: hidden;
    min-width: 0;
  }

  .tool-card-nested {
    padding-left: 16px;
  }

  .tool-header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 0;
    background: none;
    color: var(--text-muted);
    font-size: var(--size-xs);
    transition: background-color 0.1s;
    cursor: default;
    min-width: 0;
  }

  .tool-header-expandable {
    cursor: pointer;
  }

  .tool-header-expandable:hover .tool-name {
    text-decoration: underline;
  }

  .tool-caret {
    display: inline-block;
    flex-shrink: 0;
    width: 8px;
    font-size: var(--size-xs);
    color: var(--text-faint);
    transition: transform 0.15s ease;
    line-height: 1;
  }

  .tool-caret-expanded {
    transform: rotate(90deg);
  }

  .tool-caret-hidden {
    visibility: hidden;
  }

  .tool-name {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
    white-space: nowrap;
  }

  .tool-status-dot {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    height: 12px;
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .tool-status-dot.status-running {
    color: var(--ui-warning);
  }

  .tool-status-dot.status-success {
    color: var(--ui-success, var(--ui-accent));
  }

  .tool-status-dot.status-danger {
    color: var(--ui-danger);
  }

  .tool-status-dot.status-cancelled {
    color: var(--text-muted);
  }

  .tool-args-preview {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  .tool-code-block {
    background: color-mix(in srgb, var(--bg-chrome) 80%, black);
    border-radius: 8px;
    padding: 12px 14px;
    margin-top: 4px;
    max-height: 240px;
    overflow-y: auto;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.9);
    line-height: 1.5;
  }

  .tool-code-command {
    color: var(--text-primary);
    font-weight: 500;
    margin-bottom: 6px;
    white-space: pre-wrap;
    word-break: break-all;
  }

  .tool-code-output {
    margin: 0;
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .diff-output {
    color: var(--text-primary);
  }

  .tool-panel-label {
    margin: 10px 0 4px;
    color: var(--text-faint);
    font-family: inherit;
    font-size: calc(var(--size-xs) * 0.86);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .tool-panel-label:first-child {
    margin-top: 0;
  }

  .tool-meta-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin: 4px 0 8px;
  }

  .tool-chip {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    padding: 2px 6px;
    color: var(--text-muted);
    font-family: inherit;
    font-size: calc(var(--size-xs) * 0.88);
    white-space: nowrap;
  }

  .tool-code-status {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 3px;
    margin-top: 8px;
    font-size: calc(var(--size-xs) * 0.85);
    color: var(--text-muted);
  }

  .tool-code-status.status-danger {
    color: var(--ui-danger);
  }

  .tool-code-status.status-cancelled {
    color: var(--text-faint);
  }

  .tool-code-block::-webkit-scrollbar {
    width: 4px;
  }

  .tool-code-block::-webkit-scrollbar-track {
    background: transparent;
  }

  .tool-code-block::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 2px;
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

  .plan-list,
  .config-list,
  .command-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .plan-entry {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    color: var(--text-muted);
    line-height: 1.35;
  }

  .plan-status {
    flex-shrink: 0;
    min-width: 74px;
    color: var(--text-faint);
    font-size: calc(var(--size-xs) * 0.9);
    text-transform: capitalize;
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

  .note-followup-row {
    display: flex;
    justify-content: center;
    padding: 4px 0;
  }

  /* ----- Input wrapper + queue popover ----------------------------------- */

  .input-wrapper {
    flex-shrink: 0;
  }

  .queue-popover,
  .slash-command-popover {
    display: flex;
    flex-direction: column;
    gap: 0;
    margin: 0 14px;
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

  .queue-item-images img,
  .queue-item-image-placeholder {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    border: 1px solid var(--border-subtle);
  }

  .queue-item-images img {
    object-fit: cover;
  }

  .queue-item-image-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    background: var(--bg-hover);
  }

  .queue-item-error {
    color: var(--ui-danger);
    white-space: normal;
    word-break: break-word;
  }

  /* ----- Reply image previews -------------------------------------------- */

  .file-input-hidden {
    display: none;
  }

  .reply-images {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding: 8px 14px 0;
    border-top: 1px solid var(--border-subtle);
  }

  .reply-images + .input-area {
    border-top: none;
  }

  .reply-image-thumb {
    position: relative;
    width: 48px;
    height: 48px;
    border-radius: 6px;
    overflow: hidden;
    border: 1px solid var(--border-muted);
    background: var(--bg-hover);
    flex-shrink: 0;
  }

  .reply-image-thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .reply-image-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    color: var(--text-faint);
  }

  /* ----- Input area ------------------------------------------------------ */

  .input-area {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 10px 14px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    flex-shrink: 0;
  }

  .pipeline-stop-area {
    justify-content: flex-end;
  }

  .input-area :global(.message-input) {
    flex: 1;
    padding: 7px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
    resize: none;
    overflow-y: hidden;
    min-height: 36px;
    max-height: 120px;
  }

  .input-area :global(.message-input):focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  .input-area :global(.hashtag-input-wrapper) {
    flex: 1;
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

  @media (max-width: 700px) {
    .modal-header {
      padding: 12px;
    }

    .modal-content {
      padding: 12px;
    }
  }
</style>
