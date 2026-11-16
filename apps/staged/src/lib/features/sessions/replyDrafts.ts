/**
 * Reply drafts: the unsent message in a session's chat composer, persisted
 * per session under `draft:session-reply:<sessionId>` by `draftCache`.
 *
 * A note's chat is its session's chat, so this is also the note's reply
 * draft: selecting text in the note adds quote blocks to it
 * (`addQuoteToReplyDraft`, and `startLiveQuoteReply` while the reply is
 * typed in the note), and the note's row and bottom bar mark the draft while
 * it holds anything (`replyDraftPresence`). The note also marks each quoted
 * passage in its text (`replyDraftPresence.quotes`), and a marked passage
 * reopens its reply for editing (`editLiveQuoteReply`) or removal
 * (`removeQuoteFromReplyDraft`).
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
  composerDraftsEqual,
  newDraftQuoteId,
  removeDraftQuote,
  setDraftQuoteReply,
  summarizeComposerDraft,
  type ComposerDraft,
  type ComposerDraftSummary,
  type DraftQuote,
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
  /** Take the quote with this id out of the draft; nothing if it has gone already. */
  removeQuote(id: string): void;
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
 * Take a quote block out of the draft, wherever the draft is (see
 * `addQuoteToReplyDraft`). The free text around it stays, as when the
 * composer removes one. Rejects when the stored draft couldn't be written.
 */
export function removeQuoteFromReplyDraft(sessionId: string, id: string): Promise<void> {
  const live = liveComposers(sessionId);
  if (live.length > 0) {
    for (const composer of live) composer.removeQuote(id);
    return Promise.resolve();
  }
  return amendStoredReplyDraft(sessionId, (draft) => removeDraftQuote(draft, id));
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
 * rather than in the composer (see `startLiveQuoteReply` and
 * `editLiveQuoteReply`).
 */
export type LiveQuoteReply = {
  /** The block's id in the draft. */
  id: string;
  /** The reply as typed so far; written to the draft after a pause. */
  setReply(reply: string): void;
  /**
   * The field closed: write whatever is still pending. Resolves once the
   * draft holds the quote with its latest reply; rejects when it couldn't be
   * put there, so the caller can tell the user.
   */
  finish(): Promise<void>;
  /**
   * Take the block out of the draft instead, with whatever was typed.
   * Nothing typed afterwards is written. Rejects when the draft couldn't be
   * written.
   */
  remove(): Promise<void>;
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
  const id = newDraftQuoteId();
  return liveQuoteReply(
    sessionId,
    id,
    quote.reply,
    (reply) => addQuoteToReplyDraft(sessionId, { ...quote, reply }, id),
    delayMs
  );
}

/**
 * Keep the reply of a quote block already in the draft up to date as it is
 * edited in the note (its marked passage was pressed): `startLiveQuoteReply`
 * without the add, so the block is updated where it is and never doubled. A
 * block removed from the draft meanwhile stays removed.
 */
export function editLiveQuoteReply(
  sessionId: string,
  id: string,
  reply: string,
  delayMs = DRAFT_WRITE_DEBOUNCE_MS
): LiveQuoteReply {
  return liveQuoteReply(sessionId, id, reply, null, delayMs);
}

/**
 * The writer behind both: `add` puts the block in the draft first (with the
 * reply it is given), or is null for a block that is already there.
 */
function liveQuoteReply(
  sessionId: string,
  id: string,
  initialReply: string,
  add: ((reply: string) => Promise<unknown>) | null,
  delayMs: number
): LiveQuoteReply {
  let latest = initialReply;
  let added = add === null;
  let removed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  // Every write after the add, in order; a rejection anywhere is picked up
  // by `finish`, which retries from what is known to have landed.
  let chain: Promise<void> = add
    ? add(initialReply).then(() => {
        added = true;
      })
    : Promise.resolve();
  chain.catch(() => {});

  function write() {
    const reply = latest;
    chain = chain.then(() => setReplyDraftQuoteReply(sessionId, id, reply));
    chain.catch(() => {});
  }

  function cancelTimer(): boolean {
    if (!timer) return false;
    clearTimeout(timer);
    timer = null;
    return true;
  }

  return {
    id,
    setReply(reply) {
      if (removed || reply === latest) return;
      latest = reply;
      cancelTimer();
      timer = setTimeout(() => {
        timer = null;
        write();
      }, delayMs);
    },
    async finish() {
      if (removed) return;
      if (cancelTimer()) write();
      try {
        await chain;
        return;
      } catch {
        // Retried below from whatever did land.
      }
      if (removed) return;
      if (added) {
        await setReplyDraftQuoteReply(sessionId, id, latest);
      } else if (add) {
        await add(latest);
        added = true;
      }
    },
    async remove() {
      removed = true;
      cancelTimer();
      // After the writes already queued, so none of them lands on top.
      await chain.catch(() => {});
      await removeQuoteFromReplyDraft(sessionId, id);
    },
  };
}

// ---------------------------------------------------------------------------
// Which sessions have a draft
// ---------------------------------------------------------------------------

/** What a stored value holds as a draft; null for anything else, or an expired draft. */
function storedValueContent(value: unknown, now = Date.now()): DraftContent | null {
  if (!isDraftEntry(value) || value.expiresAt <= now) return null;
  return value.structured === undefined
    ? { value: value.value }
    : { value: value.value, structured: value.structured };
}

const NO_QUOTES: readonly DraftQuote[] = Object.freeze([]);

function sameQuotes(a: readonly DraftQuote[], b: readonly DraftQuote[]): boolean {
  return composerDraftsEqual({ quotes: [...a], text: '' }, { quotes: [...b], text: '' });
}

/**
 * The sessions whose reply draft holds anything, with what it holds, kept
 * current from this client's own writes (`subscribeDraftWrites`) and every
 * client's (`store://change`), and read reactively by the rows that show a
 * draft icon, the note view's draft button, and the note's text, which marks
 * the passages the draft quotes. A composer showing a draft reports it as it
 * changes (`preview`), ahead of its debounced write. `start()` once the
 * persistent store is up.
 */
class ReplyDraftPresence {
  private sessions = new SvelteMap<string, ComposerDraftSummary>();
  /** The quote blocks of each draft that has any. */
  private sessionQuotes = new SvelteMap<string, readonly DraftQuote[]>();
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

  /**
   * The quote blocks in `sessionId`'s reply draft, in order; none without a
   * draft. Reactive, and the same array until a block changes, so a reader
   * isn't re-run by the free text typed around the blocks.
   */
  quotes(sessionId: string | null | undefined): readonly DraftQuote[] {
    return (sessionId && this.sessionQuotes.get(sessionId)) || NO_QUOTES;
  }

  /**
   * The draft a composer is showing for `sessionId`, as it stands now: what
   * its next write will store. Lets a quote added to, or taken out of, an
   * open composer show in the note at once rather than a debounce later.
   */
  preview(sessionId: string, content: string | DraftContent): void {
    this.set(sessionId, typeof content === 'string' ? { value: content } : content);
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    subscribeDraftWrites((key, content) => {
      const sessionId = replyDraftSessionId(key);
      if (sessionId) this.set(sessionId, content);
    });
    listenToStoreChanges(
      (key, value) => {
        const sessionId = replyDraftSessionId(key);
        if (sessionId) this.set(sessionId, storedValueContent(value));
      },
      // Events from before the listener went live (or while a web socket was
      // down) may have been missed, so each establishment reads the store again.
      { onEstablished: () => void this.hydrate() }
    );
    // Without waiting for that: an event stream slow to connect, or that never
    // does, must not leave every draft unmarked.
    void this.hydrate();
  }

  private set(sessionId: string, content: DraftContent | null) {
    // Newer than whatever an in-flight hydrate read for this session.
    this.touchedDuringHydrate?.add(sessionId);
    this.apply(sessionId, content ? composerDraftFromContent(content) : null);
  }

  private apply(sessionId: string, draft: ComposerDraft | null) {
    const summary = draft ? summarizeComposerDraft(draft) : null;
    if (summary) this.sessions.set(sessionId, summary);
    else this.sessions.delete(sessionId);
    const quotes = draft?.quotes ?? [];
    if (quotes.length === 0) {
      this.sessionQuotes.delete(sessionId);
    } else if (!sameQuotes(this.sessionQuotes.get(sessionId) ?? NO_QUOTES, quotes)) {
      this.sessionQuotes.set(sessionId, quotes);
    }
  }

  private async hydrate(): Promise<void> {
    const generation = ++this.hydrateGeneration;
    const touched = new Set<string>();
    this.touchedDuringHydrate = touched;
    try {
      const keys = (await listStoreKeys()).filter((key) => replyDraftSessionId(key) !== null);
      const present = new Map<string, ComposerDraft>();
      for (const key of keys) {
        const content = storedValueContent(await getStoreValue<unknown>(key));
        const draft = content ? composerDraftFromContent(content) : null;
        if (draft && summarizeComposerDraft(draft)) present.set(replyDraftSessionId(key)!, draft);
      }
      if (generation !== this.hydrateGeneration) return;
      for (const sessionId of [...this.sessions.keys()]) {
        if (!present.has(sessionId) && !touched.has(sessionId)) this.apply(sessionId, null);
      }
      for (const [sessionId, draft] of present) {
        if (!touched.has(sessionId)) this.apply(sessionId, draft);
      }
    } catch (error) {
      console.error('[replyDrafts] Failed to read reply drafts:', error);
    } finally {
      if (generation === this.hydrateGeneration) this.touchedDuringHydrate = null;
    }
  }
}

export const replyDraftPresence = new ReplyDraftPresence();
