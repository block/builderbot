/**
 * Reply drafts: the unsent message in a session's chat composer, persisted
 * per session under `draft:session-reply:<sessionId>` by `draftCache`.
 *
 * A note's chat is its session's chat, so this is also the note's reply
 * draft: selecting text in the note adds quote blocks to it
 * (`addQuoteToReplyDraft`), and the note's row shows a draft icon while it
 * holds anything (`replyDraftPresence`).
 *
 * Every reply-draft write goes through the one `replyDraftWriter`, whichever
 * pane makes it, so a quote added while no composer is open can amend the
 * stored draft without racing a pane's debounced save of the same key.
 */

import { SvelteSet } from 'svelte/reactivity';
import {
  createDraftWriteScheduler,
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
  type ComposerDraft,
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

/** A composer showing a session's reply draft, which takes quotes into it live. */
export type ReplyComposerTarget = {
  addQuote(quote: NewDraftQuote): void;
};

const composers = new Map<string, ReplyComposerTarget>();

/**
 * Route quotes for `sessionId` to an open composer while it is registered.
 * The composer owns the draft while it shows it: a quote written to the store
 * underneath it would be overwritten by its next save.
 */
export function registerReplyComposer(sessionId: string, target: ReplyComposerTarget): () => void {
  composers.set(sessionId, target);
  return () => {
    if (composers.get(sessionId) === target) composers.delete(sessionId);
  };
}

/**
 * Add a quote block to the end of a session's reply draft: into the composer
 * showing it, or else into the stored draft, which a composer opened later
 * loads. The stored amend runs in `replyDraftWriter`'s queue, so a composer
 * that opens meanwhile — whose load flushes that queue first — reads it.
 */
export function addQuoteToReplyDraft(sessionId: string, quote: NewDraftQuote): Promise<void> {
  const composer = composers.get(sessionId);
  if (composer) {
    composer.addQuote(quote);
    return Promise.resolve();
  }
  return amendStoredReplyDraft(sessionId, [quote]);
}

/** Append quotes to the stored draft, with no composer involved. */
export function amendStoredReplyDraft(sessionId: string, quotes: NewDraftQuote[]): Promise<void> {
  const key = replyDraftKey(sessionId);
  return replyDraftWriter.run(async () => {
    let draft = composerDraftFromContent(await loadDraftContent(key));
    for (const quote of quotes) draft = appendDraftQuote(draft, quote);
    await saveDraft(key, composerDraftContent(draft));
  });
}

// ---------------------------------------------------------------------------
// Which sessions have a draft
// ---------------------------------------------------------------------------

function contentHasDraft(content: DraftContent | null): boolean {
  if (!content) return false;
  return content.value.trim().length > 0 || content.structured !== undefined;
}

function storedValueHasDraft(value: unknown, now = Date.now()): boolean {
  return isDraftEntry(value) && value.expiresAt > now && contentHasDraft(value);
}

/**
 * The sessions whose reply draft holds anything, kept current from this
 * client's own writes (`subscribeDraftWrites`) and every client's
 * (`store://change`), and read reactively by the rows that show a draft icon.
 * `start()` once the persistent store is up.
 */
class ReplyDraftPresence {
  private sessions = new SvelteSet<string>();
  private started = false;
  private hydrateGeneration = 0;
  /** Sessions written since the newest hydrate began reading the store. */
  private touchedDuringHydrate: Set<string> | null = null;

  /** Whether `sessionId` has a non-empty reply draft. Reactive. */
  has(sessionId: string | null | undefined): boolean {
    return !!sessionId && this.sessions.has(sessionId);
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    subscribeDraftWrites((key, content) => {
      const sessionId = replyDraftSessionId(key);
      if (sessionId) this.set(sessionId, contentHasDraft(content));
    });
    listenToStoreChanges(
      (key, value) => {
        const sessionId = replyDraftSessionId(key);
        if (sessionId) this.set(sessionId, storedValueHasDraft(value));
      },
      // Events from before the listener went live (or while a web socket was
      // down) may have been missed, so each establishment reads the store again.
      { onEstablished: () => void this.hydrate() }
    );
    // Without waiting for that: an event stream slow to connect, or that never
    // does, must not leave every draft unmarked.
    void this.hydrate();
  }

  private set(sessionId: string, present: boolean) {
    // Newer than whatever an in-flight hydrate read for this session.
    this.touchedDuringHydrate?.add(sessionId);
    if (present) this.sessions.add(sessionId);
    else this.sessions.delete(sessionId);
  }

  private async hydrate(): Promise<void> {
    const generation = ++this.hydrateGeneration;
    const touched = new Set<string>();
    this.touchedDuringHydrate = touched;
    try {
      const keys = (await listStoreKeys()).filter((key) => replyDraftSessionId(key) !== null);
      const present = new Set<string>();
      for (const key of keys) {
        if (storedValueHasDraft(await getStoreValue<unknown>(key))) {
          present.add(replyDraftSessionId(key)!);
        }
      }
      if (generation !== this.hydrateGeneration) return;
      for (const sessionId of [...this.sessions]) {
        if (!present.has(sessionId) && !touched.has(sessionId)) this.sessions.delete(sessionId);
      }
      for (const sessionId of present) {
        if (!touched.has(sessionId)) this.sessions.add(sessionId);
      }
    } catch (error) {
      console.error('[replyDrafts] Failed to read reply drafts:', error);
    } finally {
      if (generation === this.hydrateGeneration) this.touchedDuringHydrate = null;
    }
  }
}

export const replyDraftPresence = new ReplyDraftPresence();
