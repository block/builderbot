<!--
  SessionModal.svelte — View an AI session (live or historical)

  The single session viewer used everywhere in the app. Shows the message
  transcript with user messages as right-aligned chat bubbles, assistant
  messages as plain markdown, and tool calls as collapsible cards with
  argument previews.

  Features:
  - Text input always available at the bottom
  - Send button when idle, Stop button when running
  - Message queue: hitting Enter while running enqueues for after completion
  - Copy button on every message
  - Tool calls show name + args preview; expand to see output
  - Fixed-size modal with proper scrolling

  For running sessions, polls the DB incrementally:
  - Re-fetches the last known message (it may have grown from streaming)
  - Fetches any new messages after it
  - Stops polling when status leaves "running"

  Props:
    sessionId — the session to display
    onClose   — callback to close this modal
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
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Zap from '@lucide/svelte/icons/zap';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import FileText from '@lucide/svelte/icons/file-text';
  import Paperclip from '@lucide/svelte/icons/paperclip';
  import ImagePlus from '@lucide/svelte/icons/image-plus';
  import Plus from '@lucide/svelte/icons/plus';
  import Spinner from '../../shared/Spinner.svelte';
  import { marked } from 'marked';
  import { sanitize } from '../../shared/sanitize';
  import { isResumableReason } from '../../types';
  import type { Session, SessionMessage, HashtagItem, ProjectRepo } from '../../types';
  import {
    cancelSession,
    createImage,
    createImageFromData,
    deleteImage,
    getImageData,
    getSession,
    getSessionMessages,
    getSessionMessagesSince,
    handleExternalLinkClick,
    resumeSession,
  } from '../../api/commands';
  import HashtagInput from './HashtagInput.svelte';
  import {
    buildBranchHashtagItems,
    renderHashtagTokens as renderHashtagTokensShared,
  } from './hashtagItems';
  import * as Dialog from '$lib/components/ui/dialog';
  import * as Alert from '$lib/components/ui/alert';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { subscribeDragDrop } from '../branches/dragDrop';
  import {
    isImageFile,
    isMaybeTextFile,
    insertFilePathsAtCursor,
  } from '../branches/branchCardHelpers';
  import {
    groupByVerb,
    verbGroupSummary,
    hasXmlBlocks,
    stripCodeFences,
    stripXmlTags,
    type VerbGroup,
  } from './sessionModalHelpers';
  import {
    displayRootKey,
    normalizeDisplayRoots,
    resolveDisplayRoots,
    type DisplayRootInput,
  } from './pathDisplayRoots';
  import InContentSearch from '../../shared/InContentSearch.svelte';
  import PipelineSteps from './PipelineSteps.svelte';
  import { highlightMatches, clearHighlights, scrollToMatch } from '../../shared/textHighlight';
  import { registerSearchShortcutTarget } from '../keyboard/searchTargets';
  import { viewport } from '../../shared/viewport.svelte';
  import {
    buildNoteFollowupMessage,
    getNoteFollowupLabel,
    type LinkedNoteContext,
  } from './noteFreshness';

  // Configure marked
  marked.setOptions({ breaks: true, gfm: true });

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
    /** When set, shows a button to open the associated note. */
    noteInfo?: LinkedNoteContext | null;
    onOpenNote?: (note: LinkedNoteContext) => void;
  }

  let {
    open,
    sessionId,
    onClose,
    repoDir,
    branchId,
    projectId,
    repoLabel = null,
    noteInfo,
    onOpenNote,
  }: Props = $props();

  // =========================================================================
  // State
  // =========================================================================

  let session = $state<Session | null>(null);
  let messages = $state<SessionMessage[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let cancelling = $state(false);
  let messagesEl: HTMLDivElement;
  let inputEl: HTMLElement | null = $state(null);
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollInFlight = false;
  let closed = false;

  let inputText = $state('');
  let messageQueue = $state<string[]>([]);
  let copiedId = $state<number | string | null>(null);
  let expandedTools = $state<Set<number>>(new Set());
  let expandedVerbGroups = $state<Set<string>>(new Set());
  let displayRoots = $state<string[]>([]);
  let currentDisplayRootKey = '';

  let isLive = $derived(session?.status === 'running');
  let hasQueuedMessages = $derived(messageQueue.length > 0);
  let noteFollowupLabel = $derived(getNoteFollowupLabel(session, messages, noteInfo));

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
    if (branchId) {
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
    }
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
    if (!canAttachImages || isLive) return;
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
    if (!el || !canAttachImages) return;
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
  let searchVisible = $state(false);
  let searchQuery = $state('');
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let matchElements: HTMLElement[] = [];
  let unregisterSearchTarget: (() => void) | null = null;

  // =========================================================================
  // Lifecycle
  // =========================================================================

  // Register the global search-shortcut target only while the modal is open.
  // This component is mounted persistently for every branch card (the `open`
  // prop toggles visibility), and runSearchShortcut() always dispatches to the
  // last-registered target. Registering in onMount would let a closed,
  // off-screen modal capture Cmd/Ctrl+F and search next/previous, so gate
  // registration on `open` instead.
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
    closed = true;
    stopPolling();
    unregisterSearchTarget?.();
  });

  // This modal is mounted once and reused across opens (the `open` prop toggles
  // visibility), so the session must be (re)loaded reactively rather than in
  // onMount — otherwise it would load once with whatever sessionId was set at
  // mount time (often empty) and never refresh. Re-run whenever the modal is
  // opened or the target session changes.
  $effect(() => {
    // Track open + sessionId only; loadSession()'s own state writes must not
    // retrigger this effect.
    const isOpen = open;
    const id = sessionId;
    if (!isOpen || !id) {
      stopPolling();
      return;
    }
    stopPolling();
    loadSession().then(() => {
      if (closed || !open || sessionId !== id) return;
      if (session?.status === 'running') {
        startPolling();
      }
      // Focus input on open
      tick().then(() => inputEl?.focus());
    });
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
    loading = true;
    error = null;
    try {
      const [s, msgs] = await Promise.all([getSession(sessionId), getSessionMessages(sessionId)]);
      if (closed) return;
      if (!s) {
        error = 'Session not found';
        return;
      }
      session = s;
      messages = msgs;
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
      const s = await getSession(sessionId);
      if (closed) return;
      if (s) session = s;

      // Incremental message fetch
      if (messages.length === 0) {
        const msgs = await getSessionMessages(sessionId);
        if (closed) return;
        if (msgs.length > 0) {
          messages = msgs;
          scrollToBottomIfNear(true);
        }
      } else {
        const lastId = messages[messages.length - 1].id;
        const updated = await getSessionMessagesSince(sessionId, lastId);
        if (closed) return;
        if (updated.length > 0) {
          const prev = messages.slice(0, -1);
          messages = [...prev, ...updated];
          if (updated.length > 1 || updated[0].id !== lastId) {
            scrollToBottomIfNear(true);
          }
        }
      }

      // Stop polling when session is done; process queued messages
      if (s && s.status !== 'running') {
        stopPolling();
        processQueue();
      }
    } catch {
      // Polling errors are expected during shutdown — silently ignore
    } finally {
      pollInFlight = false;
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
      // Session is running — queue the message for after it finishes
      messageQueue = [...messageQueue, text];
      return;
    }

    // Session is idle — send immediately
    await sendMessage(text, imageIdsToSend);
  }

  /** Actually send a message to the backend and start the agent. */
  async function sendMessage(text: string, imageIds?: string[]) {
    if (!session || sending) return;
    sending = true;
    error = null;
    try {
      await resumeSession(session.id, text, imageIds, branchId);
      // Backend sets status to running and emits an event.
      // Force an immediate poll to pick up the new user message + status.
      session = { ...session, status: 'running' };
      startPolling();
      scrollToBottom();
    } catch (e) {
      error = `Failed to send: ${e instanceof Error ? e.message : String(e)}`;
      // Clear the queue — don't keep trying to send if the session is broken
      messageQueue = [];
    } finally {
      sending = false;
    }
  }

  function handleNoteFollowupClick() {
    if (!noteInfo || sending) return;
    void sendMessage(buildNoteFollowupMessage(noteInfo.hasParsedNote));
  }

  /** Process the next queued message when the session becomes idle. */
  async function processQueue() {
    if (messageQueue.length === 0 || isLive || error) return;
    const [next, ...rest] = messageQueue;
    messageQueue = rest;
    await sendMessage(next);
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

  function toggleTool(msgId: number) {
    const next = new Set(expandedTools);
    if (next.has(msgId)) {
      next.delete(msgId);
    } else {
      next.add(msgId);
    }
    expandedTools = next;
  }

  function toggleVerbGroup(key: string) {
    const next = new Set(expandedVerbGroups);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    expandedVerbGroups = next;
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
    return sanitize(marked.parse(content) as string);
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

  function openSearch() {
    searchVisible = true;
  }

  function closeSearch() {
    searchVisible = false;
    searchQuery = '';
    if (messagesEl) {
      clearHighlights(messagesEl);
    }
    matchCount = 0;
    currentMatchIndex = 0;
    matchElements = [];
  }

  function performSearch(query: string) {
    if (!messagesEl) return;

    // Clear previous highlights
    clearHighlights(messagesEl);
    matchElements = [];
    matchCount = 0;
    currentMatchIndex = 0;

    // If query is empty, nothing to highlight
    if (!query.trim()) return;

    // Highlight matches
    const result = highlightMatches(messagesEl, query, currentMatchIndex);
    matchElements = result.elements;
    matchCount = result.total;

    // Scroll to first match if any
    if (matchElements.length > 0) {
      scrollToMatch(matchElements[0]);
    }
  }

  function nextMatch() {
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
  }

  function previousMatch() {
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
  }

  // Re-apply search when messages change (for live sessions)
  $effect(() => {
    if (searchVisible && searchQuery.trim() && messagesEl) {
      // Debounce to avoid excessive highlighting during streaming
      const timer = setTimeout(() => {
        performSearch(searchQuery);
      }, 300);
      return () => clearTimeout(timer);
    }
  });

  function requestClose() {
    if (closed) return;
    closed = true;
    stopPolling();
    onClose();
  }

  $effect(() => {
    if (open) closed = false;
  });

  /** Group consecutive tool_call / tool_result messages into pairs */
  type ToolPair = {
    call: SessionMessage;
    result: SessionMessage | null;
  };

  type MessageGroup =
    | { type: 'user'; message: SessionMessage }
    | { type: 'assistant'; message: SessionMessage }
    | { type: 'tools'; pairs: ToolPair[] };

  let grouped = $derived.by(() => {
    const groups: MessageGroup[] = [];
    let i = 0;
    while (i < messages.length) {
      const msg = messages[i];
      if (msg.role === 'user') {
        groups.push({ type: 'user', message: msg });
        i++;
      } else if (msg.role === 'assistant') {
        groups.push({ type: 'assistant', message: msg });
        i++;
      } else {
        // tool_call / tool_result: collect into pairs
        const pairs: ToolPair[] = [];
        while (
          i < messages.length &&
          (messages[i].role === 'tool_call' || messages[i].role === 'tool_result')
        ) {
          if (messages[i].role === 'tool_call') {
            const call = messages[i];
            i++;
            // Check if next message is the matching result
            let result: SessionMessage | null = null;
            if (i < messages.length && messages[i].role === 'tool_result') {
              result = messages[i];
              i++;
            }
            pairs.push({ call, result });
          } else {
            // Orphan tool_result (shouldn't happen, but handle gracefully)
            pairs.push({
              call: messages[i],
              result: null,
            });
            i++;
          }
        }
        groups.push({ type: 'tools', pairs });
      }
    }
    return groups;
  });

  let isPipelinePrelude = $derived(isLive && !!session?.pipeline && grouped.length === 0);

  /** For each index in `grouped`, whether a user message exists at a later index.
   *  Pre-computed in O(N) so the template can do an O(1) lookup instead of
   *  scanning with `findIndex` per tool group (which was O(N²) total). */
  let hasUserAfter = $derived.by(() => {
    const arr = new Array<boolean>(grouped.length);
    let seen = false;
    for (let i = grouped.length - 1; i >= 0; i--) {
      arr[i] = seen;
      if (grouped[i].type === 'user') seen = true;
    }
    return arr;
  });

  /**
   * Pre-compute verb groups for every tools group in both tenses.
   * `groupByVerb` is expensive (JSON.parse + string replace per tool call) so we
   * cache both the past-tense and present-tense variants here. The template then
   * picks the right variant with a cheap boolean lookup instead of re-running the
   * heavy computation on every render.
   */
  let verbGroupCache = $derived.by(() => {
    const cache: { past: VerbGroup[]; present: VerbGroup[] }[] = [];
    for (const group of grouped) {
      if (group.type === 'tools') {
        cache.push({
          past: groupByVerb(group.pairs, displayRoots, true),
          present: groupByVerb(group.pairs, displayRoots, false),
        });
      } else {
        // Placeholder — tool-group index won't line up otherwise.
        // We use a separate counter in the template instead.
        cache.push({ past: [], present: [] });
      }
    }
    return cache;
  });

  /** Stable key for a message group — used to key the {#each} block for transitions.
   *  For tools groups, keys off the first pair — safe because the grouping logic
   *  in `grouped` always pushes at least one pair before creating a tools group. */
  function groupKey(group: MessageGroup): string {
    return group.type === 'tools' ? `t-${group.pairs[0].call.id}` : `m-${group.message.id}`;
  }

  /** Slide-in transition that is suppressed during the initial load. */
  function messageSlide(node: Element) {
    if (!animateNewMessages) return { duration: 0 };
    return slide(node, { duration: SLIDE_DURATION });
  }
</script>

<svelte:window onpaste={handleImagePaste} />

<Dialog.Root {open} onOpenChange={(v) => !v && requestClose()}>
  <Dialog.Content
    bind:ref={modalElement}
    class={`sm:max-w-[700px] h-[80vh] max-h-[900px] p-0 gap-0 overflow-hidden flex flex-col border-2 ${dragOver ? 'border-[var(--ui-accent)] bg-[color-mix(in_srgb,var(--ui-accent)_5%,var(--bg-chrome))]' : 'border-transparent'} transition-colors`}
    showCloseButton={false}
    onOpenAutoFocus={(e) => e.preventDefault()}
  >
    <!-- Header -->
    <header class="modal-header">
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
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="outline"
                  size="sm"
                  class="h-7 shrink-0 px-2.5 text-xs text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[700px]:h-10"
                  onclick={() => onOpenNote?.(noteInfo!)}
                >
                  View note
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Open note</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <Button
                {...props}
                variant="ghost"
                size="icon"
                class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[700px]:size-10 [&_svg]:!size-4"
                onclick={requestClose}
              >
                <X size={16} />
              </Button>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>
            {viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
          </Tooltip.Content>
        </Tooltip.Root>
      </div>
    </header>

    <!-- Messages area -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="modal-content"
      bind:this={messagesEl}
      onscroll={handleScroll}
      onclick={handleExternalLinkClick}
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
                      <Tooltip.Root>
                        <Tooltip.Trigger>
                          {#snippet child({ props })}
                            <Button
                              {...props}
                              variant="ghost"
                              size="icon"
                              class="absolute top-1.5 right-1.5 size-auto rounded p-[3px] text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-hover)] hover:text-foreground group-hover/bubble:opacity-100 [&_svg]:!size-3"
                              onclick={() => copyContent(group.message.content, group.message.id)}
                            >
                              {#if copiedId === group.message.id}
                                <Check size={12} />
                              {:else}
                                <Copy size={12} />
                              {/if}
                            </Button>
                          {/snippet}
                        </Tooltip.Trigger>
                        <Tooltip.Content>Copy message</Tooltip.Content>
                      </Tooltip.Root>
                    </div>
                  </div>
                {/if}
              {:else if group.type === 'assistant'}
                <div class="message-row assistant-message">
                  <div class="group/assistant assistant-content">
                    <div class="markdown-content">
                      {@html renderMarkdown(group.message.content)}
                    </div>
                    <Tooltip.Root>
                      <Tooltip.Trigger>
                        {#snippet child({ props })}
                          <Button
                            {...props}
                            variant="ghost"
                            size="icon"
                            class="absolute top-0 right-0 size-auto rounded p-[3px] text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-hover)] hover:text-foreground group-hover/assistant:opacity-100 [&_svg]:!size-3"
                            onclick={() => copyContent(group.message.content, group.message.id)}
                          >
                            {#if copiedId === group.message.id}
                              <Check size={12} />
                            {:else}
                              <Copy size={12} />
                            {/if}
                          </Button>
                        {/snippet}
                      </Tooltip.Trigger>
                      <Tooltip.Content>Copy message</Tooltip.Content>
                    </Tooltip.Root>
                  </div>
                </div>
              {:else}
                {@const forcePastTense = !isLive || sending || hasUserAfter[groupIdx]}
                <div class="message-row tool-group">
                  {#each forcePastTense ? verbGroupCache[groupIdx].past : verbGroupCache[groupIdx].present as vg, vgIdx}
                    {#if vg.items.length === 1}
                      {@const item = vg.items[0]}
                      {@const isExpanded = expandedTools.has(item.pair.call.id)}
                      <div class="tool-card">
                        <!-- svelte-ignore a11y_click_events_have_key_events -->
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <div
                          class="tool-header"
                          class:tool-header-expandable={!!item.pair.result}
                          onclick={() => item.pair.result && toggleTool(item.pair.call.id)}
                        >
                          <span
                            class="tool-caret"
                            class:tool-caret-expanded={isExpanded}
                            class:tool-caret-hidden={!item.pair.result}>›</span
                          >
                          <span class="tool-name">{item.verb}</span>
                          {#if item.detail}
                            <span class="tool-args-preview">{item.detail}</span>
                          {/if}
                        </div>
                        {#if isExpanded && item.pair.result}
                          {@const resultContent = stripCodeFences(item.pair.result.content)}
                          <div
                            class="tool-code-block"
                            transition:slide={{ duration: SLIDE_DURATION }}
                          >
                            {#if (item.verb === 'Ran' || item.verb === 'Running') && item.detail}
                              <div class="tool-code-command">$ {item.detail}</div>
                            {/if}
                            {#if resultContent}
                              <pre class="tool-code-output">{resultContent}</pre>
                            {/if}
                            <div class="tool-code-status">
                              <Check size={11} /> Success
                            </div>
                          </div>
                        {/if}
                      </div>
                    {:else}
                      {@const verbGroupKey = `${vg.items[0].pair.call.id}-${vg.verb}`}
                      {@const isGroupExpanded = expandedVerbGroups.has(verbGroupKey)}
                      <div class="tool-card">
                        <!-- svelte-ignore a11y_click_events_have_key_events -->
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <div
                          class="tool-header tool-header-expandable"
                          onclick={() => toggleVerbGroup(verbGroupKey)}
                        >
                          <span class="tool-caret" class:tool-caret-expanded={isGroupExpanded}
                            >›</span
                          >
                          <span class="tool-name">{vg.verb}</span>
                          <span class="tool-args-preview">{verbGroupSummary(vg)}</span>
                        </div>
                      </div>
                      {#if isGroupExpanded}
                        <div transition:slide={{ duration: SLIDE_DURATION }}>
                          {#each vg.items as item}
                            {@const isExpanded = expandedTools.has(item.pair.call.id)}
                            <div class="tool-card tool-card-nested">
                              <!-- svelte-ignore a11y_click_events_have_key_events -->
                              <!-- svelte-ignore a11y_no_static_element_interactions -->
                              <div
                                class="tool-header"
                                class:tool-header-expandable={!!item.pair.result}
                                onclick={() => item.pair.result && toggleTool(item.pair.call.id)}
                              >
                                <span
                                  class="tool-caret"
                                  class:tool-caret-expanded={isExpanded}
                                  class:tool-caret-hidden={!item.pair.result}>›</span
                                >
                                <span class="tool-name">{item.verb}</span>
                                {#if item.detail}
                                  <span class="tool-args-preview">{item.detail}</span>
                                {/if}
                              </div>
                              {#if isExpanded && item.pair.result}
                                {@const resultContent = stripCodeFences(item.pair.result.content)}
                                <div
                                  class="tool-code-block"
                                  transition:slide={{ duration: SLIDE_DURATION }}
                                >
                                  {#if (item.verb === 'Ran' || item.verb === 'Running') && item.detail}
                                    <div class="tool-code-command">$ {item.detail}</div>
                                  {/if}
                                  {#if resultContent}
                                    <pre class="tool-code-output">{resultContent}</pre>
                                  {/if}
                                  <div class="tool-code-status">
                                    <Check size={11} /> Success
                                  </div>
                                </div>
                              {/if}
                            </div>
                          {/each}
                        </div>
                      {/if}
                    {/if}
                  {/each}
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
                {#if sending}
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
              {session.completionReason === 'crashed'
                ? 'This session ended unexpectedly.'
                : session.completionReason === 'app_quit'
                  ? 'This session was interrupted when Staged closed.'
                  : 'You stopped this session.'}
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
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="destructive"
                    size="icon"
                    class="size-9 shrink-0 rounded-[10px] [&_svg]:!size-4"
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
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Stop workflow</Tooltip.Content>
          </Tooltip.Root>
        </div>
      {:else}
        {#if hasQueuedMessages}
          <div class="queue-popover">
            {#each messageQueue as msg, i}
              <div class="queue-item">
                <span class="queue-item-label">Queued</span>
                <span class="queue-item-text">{msg}</span>
                <Tooltip.Root>
                  <Tooltip.Trigger>
                    {#snippet child({ props })}
                      <Button
                        {...props}
                        variant="ghost"
                        size="icon"
                        class="size-[18px] shrink-0 rounded text-[var(--text-faint)] hover:bg-[var(--bg-hover)] hover:text-destructive [&_svg]:!size-2.5"
                        onclick={() => {
                          messageQueue = messageQueue.filter((_, idx) => idx !== i);
                        }}
                      >
                        <X size={10} />
                      </Button>
                    {/snippet}
                  </Tooltip.Trigger>
                  <Tooltip.Content>Remove from queue</Tooltip.Content>
                </Tooltip.Root>
              </div>
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
                  <Tooltip.Root>
                    <Tooltip.Trigger>
                      {#snippet child({ props })}
                        <Button
                          {...props}
                          variant="ghost"
                          size="icon"
                          class="absolute top-0.5 right-0.5 size-4 rounded-full bg-[var(--bg-deepest)] text-muted-foreground opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-chrome)] hover:text-foreground group-hover/thumb:opacity-100 [&_svg]:!size-2.5"
                          onclick={() => removeReplyImage(imageId)}
                        >
                          <X size={10} />
                        </Button>
                      {/snippet}
                    </Tooltip.Trigger>
                    <Tooltip.Content>Remove image</Tooltip.Content>
                  </Tooltip.Root>
                {/if}
              </div>
            {/each}
            {#if !isLive}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      variant="outline"
                      size="icon"
                      class="size-12 shrink-0 rounded-md border border-dashed border-[var(--border-muted)] bg-transparent text-[var(--text-faint)] shadow-none hover:border-[var(--border-emphasis)] hover:bg-transparent hover:text-muted-foreground [&_svg]:!size-4"
                      onclick={openImagePicker}
                    >
                      <Plus size={16} />
                    </Button>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>Add image</Tooltip.Content>
              </Tooltip.Root>
            {/if}
          </div>
        {/if}
        <div class="input-area">
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
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <span {...props} class="inline-flex">
                      <Button
                        variant="ghost"
                        size="icon"
                        class="size-9 shrink-0 rounded-[10px] text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
                        onclick={openImagePicker}
                        disabled={isLive}
                      >
                        <Paperclip size={16} />
                      </Button>
                    </span>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>Attach image</Tooltip.Content>
              </Tooltip.Root>
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
            <Tooltip.Root>
              <Tooltip.Trigger>
                {#snippet child({ props })}
                  <span {...props} class="inline-flex">
                    <Button
                      variant="destructive"
                      size="icon"
                      class="size-9 shrink-0 rounded-[10px] [&_svg]:!size-4"
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
                {/snippet}
              </Tooltip.Trigger>
              <Tooltip.Content>Stop session</Tooltip.Content>
            </Tooltip.Root>
          {:else}
            <Tooltip.Root>
              <Tooltip.Trigger>
                {#snippet child({ props })}
                  <span {...props} class="inline-flex">
                    <Button
                      variant="outline"
                      size="icon"
                      class="size-9 shrink-0 rounded-[10px] shadow-none disabled:opacity-30 [&_svg]:!size-4"
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
                {/snippet}
              </Tooltip.Trigger>
              <Tooltip.Content>Send message</Tooltip.Content>
            </Tooltip.Root>
          {/if}
        </div>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  /* ======================================================================= */
  /* Modal shell                                                             */
  /* ======================================================================= */

  /* ----- Header ---------------------------------------------------------- */

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

  /* ----- Content area (scrollable) --------------------------------------- */

  .modal-content {
    flex: 1;
    overflow-x: hidden;
    overflow-y: auto;
    padding: 16px;
    min-height: 0;
    /* Blend the messages-area surface halfway toward the composer's
       --bg-chrome so it reads as distinct from the input field below. */
    background: color-mix(in srgb, var(--bg-primary) 50%, var(--bg-chrome) 50%);
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
    padding-right: 28px;
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

  .tool-code-status {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 3px;
    margin-top: 8px;
    font-size: calc(var(--size-xs) * 0.85);
    color: var(--text-muted);
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

  .queue-popover {
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
    align-items: center;
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

  .queue-item-text {
    flex: 1;
    min-width: 0;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  @media (max-width: 700px) {
    .modal-header {
      padding: 12px;
    }

    .modal-content {
      padding: 12px;
    }
  }
</style>
