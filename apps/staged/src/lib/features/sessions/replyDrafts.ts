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
 * reopens its reply for editing (`editLiveQuoteReply`). A reply field that
 * closes on an empty reply takes its block out of the draft
 * (`LiveQuoteReply.discard`).
 *
 * Every reply-draft write goes through the one `replyDraftWriter`, whichever
 * pane makes it, so a quote added while no composer is open can amend the
 * stored draft without racing a pane's debounced save of the same key.
 */

import { untrack } from 'svelte';
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
  cloneComposerDraft,
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
  /**
   * Take the quote with this id out of the draft; nothing if it has gone
   * already, or if `when` is given and does not hold of the block as the
   * composer has it.
   */
  removeQuote(id: string, when?: (quote: DraftQuote) => boolean): void;
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
 * composer removes one. With `when`, only a block it holds of as the draft
 * has it at the removal, not as the caller last saw it. Rejects when the
 * stored draft couldn't be written.
 */
export function removeQuoteFromReplyDraft(
  sessionId: string,
  id: string,
  when?: (quote: DraftQuote) => boolean
): Promise<void> {
  const live = liveComposers(sessionId);
  if (live.length > 0) {
    for (const composer of live) composer.removeQuote(id, when);
    return Promise.resolve();
  }
  return amendStoredReplyDraft(sessionId, (draft) => removeDraftQuote(draft, id, when));
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
  /**
   * The reply as typed so far; written to the draft after a pause. Not an
   * empty one (nothing, or only whitespace): that waits for the field to
   * close, which takes the block out (`discard`) or, failing that, writes
   * it (`finish`). A field emptied and then cut off before it closes (the
   * page reloaded, the app quit) so leaves the block with the reply it last
   * had, not as an empty block nothing in the note would delete.
   */
  setReply(reply: string): void;
  /**
   * The field closed: write whatever is still pending. Resolves once the
   * draft holds the quote with its latest reply; rejects when it couldn't be
   * put there, so the caller can tell the user.
   */
  finish(): Promise<void>;
  /**
   * The field closed on an empty reply: take the block out of the draft
   * instead, with whatever was typed and not yet written. Nothing typed
   * afterwards is written. A block whose reply in the draft is not this
   * field's doing stays: it is removed only while that reply is empty, or
   * is the one this field opened on or last wrote, so a reply given to the
   * block meanwhile in a composer, or by another client, is not deleted
   * with it. Rejects when the draft couldn't be written.
   */
  discard(): Promise<void>;
};

/**
 * Add a quote to a session's reply draft and keep its reply up to date as it
 * is typed, so the field it is typed in has nothing to confirm: dismissing
 * the field just writes what is left (`finish`), or takes the block out
 * again when nothing was written in it (`discard`). The block goes into the
 * draft at once, with the reply it starts with (usually none), and each
 * pause in typing writes the reply so far (unless it is empty: see
 * `LiveQuoteReply.setReply`) through the shared writer — into
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
 * Where the live writes to each block have got to, by session and block. A
 * field reopened on a block while the field before it is still writing
 * queues behind those writes: its `discard` goes by the reply the draft
 * holds, which must not be one the earlier field is about to replace, or
 * the block would stay and that later write would land in it.
 */
const liveWrites = new Map<string, Promise<void>>();

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
  const writesKey = `${sessionId}\n${id}`;
  let latest = initialReply;
  /** The reply the draft holds for the block, as far as this field's writes go. */
  let written = initialReply;
  let added = add === null;
  let discarded = false;
  /** `latest` is yet to be handed to a write: by `timer`, or at the close when it is empty. */
  let unwritten = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  // Every write after the add, in order; a rejection anywhere is picked up
  // by `finish`, which retries from what is known to have landed.
  let chain: Promise<void> = Promise.resolve();

  /** `next` is where this block's writes have got to (see `liveWrites`). */
  function follow(next: Promise<void>) {
    chain = next;
    liveWrites.set(writesKey, next);
    const settled = () => {
      if (liveWrites.get(writesKey) === next) liveWrites.delete(writesKey);
    };
    next.then(settled, settled);
  }

  follow(
    add
      ? add(initialReply).then(() => {
          added = true;
        })
      : // Behind whatever a field on this block before this one is yet to write.
        (liveWrites.get(writesKey) ?? Promise.resolve()).catch(() => {})
  );

  function write() {
    const reply = latest;
    unwritten = false;
    follow(
      chain
        .then(() => setReplyDraftQuoteReply(sessionId, id, reply))
        .then(() => {
          written = reply;
        })
    );
  }

  function cancelTimer() {
    if (timer) clearTimeout(timer);
    timer = null;
  }

  return {
    id,
    setReply(reply) {
      if (discarded || reply === latest) return;
      latest = reply;
      unwritten = true;
      cancelTimer();
      // An empty reply is the close's to deal with (see LiveQuoteReply.setReply).
      if (reply.trim() === '') return;
      timer = setTimeout(() => {
        timer = null;
        write();
      }, delayMs);
    },
    async finish() {
      if (discarded) return;
      cancelTimer();
      if (unwritten) write();
      try {
        await chain;
        return;
      } catch {
        // Retried below from whatever did land.
      }
      if (discarded) return;
      if (added) {
        await setReplyDraftQuoteReply(sessionId, id, latest);
      } else if (add) {
        await add(latest);
        added = true;
      }
    },
    async discard() {
      if (discarded) return;
      discarded = true;
      // What was typed and not yet written goes with the block.
      cancelTimer();
      // After the writes already queued, so none of them lands on top, and
      // judged on the draft as it is then (see LiveQuoteReply.discard).
      const removal = chain
        .catch(() => {})
        .then(() =>
          removeQuoteFromReplyDraft(
            sessionId,
            id,
            (quote) => quote.reply.trim() === '' || quote.reply === written
          )
        );
      follow(removal);
      await removal;
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

function sameSummary(a: ComposerDraftSummary | undefined, b: ComposerDraftSummary): boolean {
  return !!a && a.quotes === b.quotes && a.hasText === b.hasText;
}

/**
 * The sessions whose reply draft holds anything, with what it holds, kept
 * current from this client's own writes (`subscribeDraftWrites`) and every
 * client's (`store://change`), and read reactively by the rows that show a
 * draft icon, the note view's draft button, and the note's text, which marks
 * the passages the draft quotes. A composer showing a draft reports it as it
 * changes (`preview`), ahead of its debounced write. `start()` once the
 * persistent store is up.
 *
 * Writing here never subscribes the writer: what a write compares against is
 * read untracked, so an effect that reports a draft is not re-run by the
 * next report of the same session's, from another composer or another
 * client. And a write that changes nothing a reader sees re-runs no reader:
 * the summary and the quotes are replaced only when they differ.
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
   * The draft itself, not its stored form, which every keystroke would have
   * to be parsed back out of; nothing of it is kept but copies.
   */
  preview(sessionId: string, draft: ComposerDraft): void {
    this.touchedDuringHydrate?.add(sessionId);
    this.apply(sessionId, draft);
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
    // Untracked: see the class comment.
    untrack(() => {
      const summary = draft ? summarizeComposerDraft(draft) : null;
      if (!summary) {
        this.sessions.delete(sessionId);
      } else if (!sameSummary(this.sessions.get(sessionId), summary)) {
        this.sessions.set(sessionId, summary);
      }
      const quotes = draft?.quotes ?? [];
      if (quotes.length === 0) {
        this.sessionQuotes.delete(sessionId);
      } else if (!sameQuotes(this.sessionQuotes.get(sessionId) ?? NO_QUOTES, quotes)) {
        // Copies: a composer's own draft is live state it goes on editing.
        this.sessionQuotes.set(
          sessionId,
          cloneComposerDraft({ quotes: [...quotes], text: '' }).quotes
        );
      }
    });
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
