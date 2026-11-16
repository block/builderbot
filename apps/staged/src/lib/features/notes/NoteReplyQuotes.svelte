<!--
  NoteReplyQuotes.svelte — a note's quoted replies, in the note.

  The note's chat draft (sessions/replyDrafts.ts) holds a quote block for
  each passage the reader replied to. This puts the two halves of that in
  the rendered note and joins them to the draft:

  - `NoteSelectionReply`: selecting text offers a Reply button, and the
    reply lands in the draft as a new block — straight into the composer
    when the chat shows beside the note (`chatVisible`), otherwise typed in
    a field by the selection and written as it goes.
  - `NoteQuoteHighlights`: every passage the draft quotes stays marked for
    as long as its block is in the draft, and pressing a mark goes back to
    its reply. Beside the note the composer has the block, so the caret
    lands in its reply there; otherwise the reply field reopens by the
    passage with what was written, to edit. A field closed on an empty
    reply takes its block out of the draft, and the mark with it: that is
    how a quote is deleted from the note.

  The two meet where a selection is exactly a passage that is already
  quoted: Reply then goes back to that block's reply, as a press on its mark
  does, rather than adding a second block on the same words.

  The draft is read through `replyDraftPresence`, which follows writes as
  they land. Two things keep the field honest about that: a reply typed in
  the field is remembered here until the draft shows it (`heldReplies`), so
  a field closed and reopened at once still holds it; and a block that
  leaves the draft while its field is open closes the field.

  Rendered inside the note's scroll container, as NoteSelectionReply must
  be. A draft write that fails is reported in a toast.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { toast } from 'svelte-sonner';
  import type { DraftQuote, NewDraftQuote } from '../sessions/composerDraft';
  import {
    addQuoteToReplyDraft,
    editLiveQuoteReply,
    focusReplyDraftQuote,
    replyDraftPresence,
    startLiveQuoteReply,
    type LiveQuoteReply,
  } from '../sessions/replyDrafts';
  import NoteQuoteHighlights from './NoteQuoteHighlights.svelte';
  import { quoteAtPassage } from './noteQuoteLocation';
  import NoteSelectionReply from './NoteSelectionReply.svelte';

  interface Props {
    /** The session whose chat the note is replied to in. */
    sessionId: string;
    /** The note's scroll container (positioned); the Reply button sits in it. */
    scrollEl: HTMLElement | undefined;
    /** The rendered note. */
    contentEl: HTMLElement | undefined;
    /** What the note was rendered from: its marks are put back on every new render. */
    renderKey: unknown;
    /** Bumped when something else rewrote the note's text under the marks (find-in-note). */
    refreshKey?: unknown;
    /** Off while the note is hidden or not rendered. */
    enabled: boolean;
    /** The chat is open, beside the note or over it. */
    chatOpen: boolean;
    /** The chat composer shows the draft beside the note. */
    chatVisible: boolean;
    /**
     * The dialog around the note: a phone's reply sheet goes in it, and it
     * takes focus from a closing field with no mark to give it to. And on a
     * phone, where the docked Reply button goes (see NoteSelectionReply).
     */
    sheetHost?: HTMLElement | null;
    dockHost?: HTMLElement | null;
    onDockedChange?: (docked: boolean) => void;
    /** Headings left out of a quote's heading path (the note's title). */
    ignoreHeading?: (heading: Element) => boolean;
  }

  let {
    sessionId,
    scrollEl,
    contentEl,
    renderKey,
    refreshKey = null,
    enabled,
    chatOpen,
    chatVisible,
    sheetHost = null,
    dockHost = null,
    onDockedChange,
    ignoreHeading,
  }: Props = $props();

  /** The draft's quote blocks, whose passages the note marks. */
  let quotes = $derived(replyDraftPresence.quotes(sessionId));
  /** The quotes the note found and marked, and the one whose reply field is open. */
  let markedQuoteIds = $state.raw<string[]>([]);
  let activeQuoteId = $state<string | null>(null);
  let selectionReply = $state<ReturnType<typeof NoteSelectionReply>>();

  /**
   * A reply as the note's field last held it, for a block whose draft may
   * not show it yet. The draft's write is queued behind a read of the store
   * and is not waited for when the field closes, so a mark pressed straight
   * away (Enter, then Enter: focus is back on the mark) would reopen on the
   * reply from before, and what is typed next would be written over the
   * newer text.
   */
  type HeldReply = {
    reply: string;
    /**
     * Set once the field's writes have ended: what the draft showed for the
     * block then. From there the draft is the newer of the two as soon as
     * it shows anything else, the block gone included.
     */
    writtenOver?: { reply: string };
  };
  /**
   * Held for a block only while its field's writes are under way, or the
   * block is in the draft: never more entries than open fields and blocks.
   */
  const heldReplies = new Map<string, HeldReply>();

  /** How many replies are held. For tests. */
  export function heldReplyCount(): number {
    return heldReplies.size;
  }

  function draftReply(quoteId: string): string | undefined {
    return quotes.find((quote) => quote.id === quoteId)?.reply;
  }

  /** `quote` with the reply its field last held, where the draft is yet to show it. */
  function withHeldReply(quote: DraftQuote): DraftQuote {
    const held = heldReplies.get(quote.id);
    return held && held.reply !== quote.reply ? { ...quote, reply: held.reply } : quote;
  }

  // A held reply is let go once the draft shows it, or has moved on since
  // the field's writes ended.
  $effect(() => {
    // Read whether or not anything is held now, so the effect runs again
    // for a reply held later.
    const shownQuotes = quotes;
    for (const [quoteId, held] of heldReplies) {
      const shown = shownQuotes.find((quote) => quote.id === quoteId)?.reply;
      if (shown === held.reply || (held.writtenOver && held.writtenOver.reply !== shown)) {
        heldReplies.delete(quoteId);
      }
    }
  });

  /**
   * The block the open field writes to has left the draft: removed in the
   * composer or sent, by another window or another client. The field
   * closes: left open it would take a reply that goes nowhere. Only a block
   * the draft has shown, which a new reply's is not until its add lands.
   */
  let shownActiveQuoteId: string | null = null;
  $effect(() => {
    const quoteId = activeQuoteId;
    if (quoteId === null) {
      shownActiveQuoteId = null;
    } else if (quotes.some((quote) => quote.id === quoteId)) {
      shownActiveQuoteId = quoteId;
    } else if (shownActiveQuoteId === quoteId) {
      shownActiveQuoteId = null;
      untrack(() => selectionReply?.closeQuote(quoteId));
    }
  });

  /**
   * The block already quoting exactly the passage `quote` was just selected
   * at, if there is one: the passage is then replied to there (see the
   * header).
   */
  function existingQuoteFor(quote: NewDraftQuote): DraftQuote | null {
    if (!contentEl) return null;
    const existing = quoteAtPassage(contentEl, quotes, quote, { ignore: ignoreHeading });
    return existing ? withHeldReply(existing) : null;
  }

  /**
   * With the composer showing beside the note, a selected passage goes
   * straight into it as a quote block and the caret lands in the block's
   * reply; in the block it already has, when the passage is quoted already.
   * Returns whether the selection is dealt with: on failure the Reply
   * button stays for another try.
   */
  async function handleQuoteAdd(quote: NewDraftQuote): Promise<boolean> {
    const sid = sessionId;
    const existing = existingQuoteFor(quote);
    if (existing) {
      focusReplyDraftQuote(sid, existing.id);
      return true;
    }
    let id: string;
    try {
      id = await addQuoteToReplyDraft(sid, quote);
    } catch (error) {
      console.error('Failed to add quote to reply draft', error);
      toast.error('Could not add the quote to your reply draft');
      return false;
    }
    focusReplyDraftQuote(sid, id);
    return true;
  }

  /**
   * With the chat hidden, the reply to a selected passage is typed in the
   * note and the draft follows it (see replyDrafts.startLiveQuoteReply).
   * Closing the field leaves the user on the note, whatever the width, so
   * the next passage can be selected without the view changing underfoot;
   * the note's bottom bar offers the chat while the draft holds anything.
   */
  function handleQuoteReply(quote: NewDraftQuote): LiveQuoteReply {
    return reporting(startLiveQuoteReply(sessionId, quote));
  }

  /**
   * The same field reopened on a block already in the draft (its marked
   * passage was pressed): what is typed updates that block.
   */
  function handleQuoteEdit(quote: DraftQuote): LiveQuoteReply {
    return reporting(editLiveQuoteReply(sessionId, quote.id, quote.reply));
  }

  /**
   * `live`, telling the user when the draft couldn't be written, and
   * holding the reply typed into it until the draft shows it (`heldReplies`).
   */
  function reporting(live: LiveQuoteReply): LiveQuoteReply {
    const quoteId = live.id;
    return {
      id: quoteId,
      setReply: (reply) => {
        heldReplies.set(quoteId, { reply });
        live.setReply(reply);
      },
      finish: async () => {
        // Whatever this field, or one opened on the block before it, typed.
        const held = heldReplies.get(quoteId);
        try {
          await live.finish();
        } catch (error) {
          console.error('Failed to save the reply draft', error);
          toast.error('Could not save your reply draft');
          // Not kept after all: the field reopens on what the draft has.
          if (held && heldReplies.get(quoteId) === held) heldReplies.delete(quoteId);
          return;
        }
        if (!held || heldReplies.get(quoteId) !== held) return;
        const shown = draftReply(quoteId);
        // Shown, or with no block left to show it in: removed or sent from
        // elsewhere while it was typed, so nothing will ever let it go.
        if (shown === undefined || shown === held.reply) heldReplies.delete(quoteId);
        else held.writtenOver = { reply: shown };
      },
      discard: async () => {
        // Nothing of it is left to show: the block is going.
        heldReplies.delete(quoteId);
        try {
          await live.discard();
        } catch (error) {
          console.error('Failed to remove the quote from the reply draft', error);
          toast.error('Could not remove the quote from your reply draft');
        }
      },
    };
  }

  /**
   * A marked passage was pressed (`stop` is its keyboard stop, for a key
   * press): back to its reply, wherever that is written.
   */
  function handleMarkActivate(quoteId: string, range: Range, stop: HTMLElement | null) {
    const quote = quotes.find((candidate) => candidate.id === quoteId);
    if (!quote) return;
    if (chatVisible) {
      focusReplyDraftQuote(sessionId, quoteId);
      return;
    }
    selectionReply?.editQuote(withHeldReply(quote), range, stop !== null);
  }
</script>

<NoteQuoteHighlights
  {contentEl}
  {renderKey}
  {refreshKey}
  {quotes}
  {activeQuoteId}
  {ignoreHeading}
  onMarked={(quoteIds) => (markedQuoteIds = quoteIds)}
  onActivate={handleMarkActivate}
/>
<NoteSelectionReply
  bind:this={selectionReply}
  {scrollEl}
  {contentEl}
  {enabled}
  {chatOpen}
  {chatVisible}
  {sheetHost}
  {dockHost}
  {onDockedChange}
  {ignoreHeading}
  onAdd={handleQuoteAdd}
  onReply={handleQuoteReply}
  onEdit={handleQuoteEdit}
  existingQuote={existingQuoteFor}
  {markedQuoteIds}
  onActiveQuoteChange={(quoteId) => (activeQuoteId = quoteId)}
/>
