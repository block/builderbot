<!--
  Test host for ChatComposer: keeps the draft the composer edits, as
  SessionChatPane does, so tests can watch a quote leave the draft and come
  back. `onRemoveQuote` takes the quote out, merging the text around it, and
  hands back the restore the composer's undo runs; `refuseUndo` makes that
  restore answer no, as the pane's does once the draft has been sent or
  another session is in the box. `holdUndo` makes it accept the undo without
  carrying it out, as the pane does while its draft is loading: the quote
  comes back on `landHeldUndo`, unless the composer's redo withdrew the undo
  first.
-->
<script lang="ts">
  import ChatComposer from '../ChatComposer.svelte';
  import { composerSendControl } from '../composerSendControl';
  import {
    cloneComposerDraft,
    restoreDraftQuote,
    takeDraftQuote,
    type ComposerDraft,
    type DraftQuote,
  } from '../composerDraft';

  let quotes = $state<DraftQuote[]>([]);
  let text = $state('');
  let refusing = false;
  let holding = false;
  /** The undos accepted while holding, each still to be carried out. */
  let held: Array<() => void> = [];
  let removed: string[] = [];

  const sendControl = composerSendControl({
    isLive: false,
    canSendLive: false,
    mode: 'queue',
    canStartNewSession: false,
  });

  function removeQuote(id: string) {
    const taken = takeDraftQuote({ quotes, text }, id);
    if (!taken) return null;
    removed.push(id);
    quotes = taken.draft.quotes;
    text = taken.draft.text;
    let heldRestore: (() => void) | null = null;
    return {
      restore(then: () => void) {
        if (refusing) return false;
        const apply = () => {
          const draft = restoreDraftQuote({ quotes, text }, taken.removed);
          quotes = draft.quotes;
          text = draft.text;
          then();
        };
        if (holding) {
          heldRestore = apply;
          held.push(apply);
        } else {
          apply();
        }
        return true;
      },
      withdraw() {
        const index = heldRestore ? held.indexOf(heldRestore) : -1;
        heldRestore = null;
        if (index === -1) return false;
        held.splice(index, 1);
        return true;
      },
    };
  }

  export function setDraft(draft: ComposerDraft) {
    quotes = draft.quotes;
    text = draft.text;
  }

  /** The draft as it stands, as plain data. */
  export function draft(): ComposerDraft {
    return cloneComposerDraft({ quotes, text });
  }

  /** The ids `onRemoveQuote` took out, in order. */
  export function removedIds(): string[] {
    return [...removed];
  }

  export function refuseUndo(value: boolean) {
    refusing = value;
  }

  export function holdUndo(value: boolean) {
    holding = value;
  }

  /** Carry out the undos accepted while holding that were not withdrawn since. */
  export function landHeldUndo() {
    const landing = held;
    held = [];
    for (const apply of landing) apply();
  }
</script>

<ChatComposer bind:value={text} bind:quotes onRemoveQuote={removeQuote} {sendControl} />
