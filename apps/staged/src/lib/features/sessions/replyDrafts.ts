/**
 * Reply drafts: the unsent message in a session's chat composer, persisted
 * per session under `draft:session-reply:<sessionId>` by `draftCache`.
 *
 * A note's chat is its session's chat, so this is also the note's reply
 * draft: selecting text in the note adds quote blocks to it
 * (`addQuoteToReplyDraft`, and `startLiveQuoteReply` while the reply is
 * typed in the note), and the note's row and bottom bar mark the draft while
 * it holds anything (`replyDraftPresence`).
 *
 * Every reply-draft write goes through the one `replyDraftWriter`, whichever
 * pane makes it, so a quote added while no composer is open can amend the
 * stored draft without racing a pane's debounced save of the same key.
 */

import { SvelteMap } from 'svelte/reactivity';
import {
  createDraftWriteScheduler,
  DRAFT_WRITE_DEBOUNCE_MS,
  isDraftEntry,
  loadDraftContent,
  saveDraft,
  subscribeDraftWrites,
  type DraftContent,
} from '../../shared/draftCache';
import { getStoreValue, listenToStoreChanges, listStoreKeys } from '../../shared/persistentStore';
import {
  appendDraftQuote,
  composerDraftContent,
  composerDraftFromContent,
  newDraftQuoteId,
  setDraftQuoteReply,
  summarizeComposerDraft,
  type ComposerDraft,
  type ComposerDraftSummary,
  type NewDraftQuote,
} from './composerDraft';

const REPLY_DRAFT_KEY_PREFIX = 'draft:session-reply:';

export function replyDraftKey(sessionId: string): string {
  return `${REPLY_DRAFT_KEY_PREFIX}${sessionId}`;
}

function replyDraftSessionId(key: string): string | null {
  return key.startsWith(REPLY_DRAFT_KEY_PREFIX) ? key.slice(REPLY_DRAFT_KEY_PREFIX.length) : null;
}

/** The writer every reply draft is saved through (see the module comment). */
export const replyDraftWriter = createDraftWriteScheduler();

/** A session's stored reply draft; an empty draft when there is none. */
export async function loadReplyDraft(sessionId: string): Promise<ComposerDraft> {
  return composerDraftFromContent(await loadDraftContent(replyDraftKey(sessionId)));
}

// ---------------------------------------------------------------------------
// Adding quotes
// ---------------------------------------------------------------------------

/** A composer showing a session's reply draft, which takes quote edits into it live. */
export type ReplyComposerTarget = {
  /** Append a quote block with this id (the caller's, so it can be edited later). */
  addQuote(quote: NewDraftQuote, id: string): void;
  /** Replace the reply of the quote with this id; nothing if it has been removed. */
  setQuoteReply(id: string, reply: string): void;
  /** Put the caret in that quote's reply input, if the block is shown. */
  focusQuoteReply(id: string): void;
};

/**
 * The composers showing each session's draft, oldest first. Usually one; two
 * dialogs open on the same session (`sessionPanePool.acquire` mounts a
 * transient pane beside the pooled one) register two, and each keeps its own
 * copy of the draft and saves it in turn, so an edit routed to only one
 * would be overwritten by the other's next save. Edits therefore go to every
 * registered composer; the caret goes to the newest, the one just opened.
 */
const composers = new Map<string, ReplyComposerTarget[]>();

/**
 * Route quote edits for `sessionId` to an open composer while it is
 * registered. The composer owns the draft while it shows it: a quote written
 * to the store underneath it would be overwritten by its next save. A
 * composer that unregisters leaves any others for the session registered.
 */
export function registerReplyComposer(sessionId: string, target: ReplyComposerTarget): () => void {
  const targets = composers.get(sessionId) ?? [];
  composers.set(sessionId, [...targets, target]);
  return () => {
    const remaining = (composers.get(sessionId) ?? []).filter((t) => t !== target);
    if (remaining.length > 0) composers.set(sessionId, remaining);
    else composers.delete(sessionId);
  };
}

/** The composers showing `sessionId`'s draft, oldest first; empty when none does. */
function liveComposers(sessionId: string): ReplyComposerTarget[] {
  return composers.get(sessionId) ?? [];
}

/**
 * Add a quote block to the end of a session's reply draft: into the composer
 * showing it, or else into the stored draft, which a composer opened later
 * loads. The stored amend runs in `replyDraftWriter`'s queue, so a composer
 * that opens meanwhile — whose load flushes that queue first — reads it.
 *
 * Resolves with the block's id once the quote is in the draft, so its reply
 * can be edited afterwards (`setReplyDraftQuoteReply`), and rejects when the
 * stored draft couldn't be written, so the caller can tell the user.
 */
export function addQuoteToReplyDraft(
  sessionId: string,
  quote: NewDraftQuote,
  id = newDraftQuoteId()
): Promise<string> {
  const live = liveComposers(sessionId);
  if (live.length > 0) {
    for (const composer of live) composer.addQuote(quote, id);
    return Promise.resolve(id);
  }
  return amendStoredReplyDraft(sessionId, (draft) => appendDraftQuote(draft, quote, id)).then(
    () => id
  );
}

/**
 * Replace the reply of a quote block already in the draft, wherever the
 * draft is (see `addQuoteToReplyDraft`). A block the user has since removed
 * is left removed. Rejects when the stored draft couldn't be written.
 */
export function setReplyDraftQuoteReply(
  sessionId: string,
  id: string,
  reply: string
): Promise<void> {
  const live = liveComposers(sessionId);
  if (live.length > 0) {
    for (const composer of live) composer.setQuoteReply(id, reply);
    return Promise.resolve();
  }
  return amendStoredReplyDraft(sessionId, (draft) => setDraftQuoteReply(draft, id, reply));
}

/**
 * Put the caret in a quote block's reply input. Only a composer showing the
 * draft can; returns whether one did. With several showing it, the newest
 * registered — the one opened last — takes the caret.
 */
export function focusReplyDraftQuote(sessionId: string, id: string): boolean {
  const composer = liveComposers(sessionId).at(-1);
  if (!composer) return false;
  composer.focusQuoteReply(id);
  return true;
}

/**
 * Change the stored draft, with no composer involved: `amend` gets the
 * stored draft and returns the one to store. Rejects if the write fails.
 */
export function amendStoredReplyDraft(
  sessionId: string,
  amend: (draft: ComposerDraft) => ComposerDraft
): Promise<void> {
  const key = replyDraftKey(sessionId);
  return replyDraftWriter.run(async () => {
    const draft = amend(composerDraftFromContent(await loadDraftContent(key)));
    await saveDraft(key, composerDraftContent(draft));
  });
}

// ---------------------------------------------------------------------------
// A reply typed in the note
// ---------------------------------------------------------------------------

/**
 * A quote block whose reply is being typed in the note's reply field,
 * rather than in the composer (see `startLiveQuoteReply`).
 */
export type LiveQuoteReply = {
  /** The reply as typed so far; written to the draft after a pause. */
  setReply(reply: string): void;
  /**
   * The field closed: write whatever is still pending. Resolves once the
   * draft holds the quote with its latest reply; rejects when it couldn't be
   * put there, so the caller can tell the user.
   */
  finish(): Promise<void>;
};

/**
 * Add a quote to a session's reply draft and keep its reply up to date as it
 * is typed, so the field it is typed in has nothing to confirm: dismissing
 * the field (`finish`) just writes what is left. The block goes into the
 * draft at once, with the reply it starts with (usually none), and each
 * pause in typing writes the reply so far through the shared writer — into
 * the composer, if one has opened the chat meanwhile, by the id the block
 * was added under, so the block is never added twice.
 *
 * A write that fails is retried from `finish` with the whole quote, so a
 * reply typed over a store hiccup still lands or is reported.
 */
export function startLiveQuoteReply(
  sessionId: string,
  quote: NewDraftQuote,
  delayMs = DRAFT_WRITE_DEBOUNCE_MS
): LiveQuoteReply {
  let latest = quote.reply;
  let addedId: string | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  // Every write after the add, in order; a rejection anywhere is picked up
  // by `finish`, which retries from what is known to have landed.
  let chain: Promise<string> = addQuoteToReplyDraft(sessionId, quote).then((id) => {
    addedId = id;
    return id;
  });
  chain.catch(() => {});

  function write() {
    const reply = latest;
    chain = chain.then(async (id) => {
      await setReplyDraftQuoteReply(sessionId, id, reply);
      return id;
    });
    chain.catch(() => {});
  }

  return {
    setReply(reply) {
      if (reply === latest) return;
      latest = reply;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        write();
      }, delayMs);
    },
    async finish() {
      if (timer) {
        clearTimeout(timer);
        timer = null;
        write();
      }
      try {
        await chain;
        return;
      } catch {
        // Retried below from whatever did land.
      }
      if (addedId !== null) await setReplyDraftQuoteReply(sessionId, addedId, latest);
      else addedId = await addQuoteToReplyDraft(sessionId, { ...quote, reply: latest });
    },
  };
}

// ---------------------------------------------------------------------------
// Which sessions have a draft
// ---------------------------------------------------------------------------

function contentSummary(content: DraftContent | null): ComposerDraftSummary | null {
  if (!content) return null;
  return summarizeComposerDraft(composerDraftFromContent(content));
}

function storedValueSummary(value: unknown, now = Date.now()): ComposerDraftSummary | null {
  return isDraftEntry(value) && value.expiresAt > now ? contentSummary(value) : null;
}

/**
 * The sessions whose reply draft holds anything, with what it holds, kept
 * current from this client's own writes (`subscribeDraftWrites`) and every
 * client's (`store://change`), and read reactively by the rows that show a
 * draft icon and the note view's draft button. `start()` once the persistent
 * store is up.
 */
class ReplyDraftPresence {
  private sessions = new SvelteMap<string, ComposerDraftSummary>();
  private started = false;
  private hydrateGeneration = 0;
  /** Sessions written since the newest hydrate began reading the store. */
  private touchedDuringHydrate: Set<string> | null = null;

  /** Whether `sessionId` has a non-empty reply draft. Reactive. */
  has(sessionId: string | null | undefined): boolean {
    return !!sessionId && this.sessions.has(sessionId);
  }

  /** What `sessionId`'s reply draft holds; null without one. Reactive. */
  summary(sessionId: string | null | undefined): ComposerDraftSummary | null {
    return (sessionId && this.sessions.get(sessionId)) || null;
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    subscribeDraftWrites((key, content) => {
      const sessionId = replyDraftSessionId(key);
      if (sessionId) this.set(sessionId, contentSummary(content));
    });
    listenToStoreChanges(
      (key, value) => {
        const sessionId = replyDraftSessionId(key);
        if (sessionId) this.set(sessionId, storedValueSummary(value));
      },
      // Events from before the listener went live (or while a web socket was
      // down) may have been missed, so each establishment reads the store again.
      { onEstablished: () => void this.hydrate() }
    );
    // Without waiting for that: an event stream slow to connect, or that never
    // does, must not leave every draft unmarked.
    void this.hydrate();
  }

  private set(sessionId: string, summary: ComposerDraftSummary | null) {
    // Newer than whatever an in-flight hydrate read for this session.
    this.touchedDuringHydrate?.add(sessionId);
    if (summary) this.sessions.set(sessionId, summary);
    else this.sessions.delete(sessionId);
  }

  private async hydrate(): Promise<void> {
    const generation = ++this.hydrateGeneration;
    const touched = new Set<string>();
    this.touchedDuringHydrate = touched;
    try {
      const keys = (await listStoreKeys()).filter((key) => replyDraftSessionId(key) !== null);
      const present = new Map<string, ComposerDraftSummary>();
      for (const key of keys) {
        const summary = storedValueSummary(await getStoreValue<unknown>(key));
        if (summary) present.set(replyDraftSessionId(key)!, summary);
      }
      if (generation !== this.hydrateGeneration) return;
      for (const sessionId of [...this.sessions.keys()]) {
        if (!present.has(sessionId) && !touched.has(sessionId)) this.sessions.delete(sessionId);
      }
      for (const [sessionId, summary] of present) {
        if (!touched.has(sessionId)) this.sessions.set(sessionId, summary);
      }
    } catch (error) {
      console.error('[replyDrafts] Failed to read reply drafts:', error);
    } finally {
      if (generation === this.hydrateGeneration) this.touchedDuringHydrate = null;
    }
  }
}

export const replyDraftPresence = new ReplyDraftPresence();
