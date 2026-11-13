<!--
  Test host for ChatComposer: keeps the draft the composer edits, as
  SessionChatPane does, so tests can watch a quote leave the draft and come
  back. `onRemoveQuote` takes the quote out, merging the text around it, and
  hands back the restore the composer's undo runs; `refuseUndo` makes that
  restore answer no, as the pane's does once the draft has been sent or
  another session is in the box.
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
    return (then: () => void) => {
      if (refusing) return false;
      const draft = restoreDraftQuote({ quotes, text }, taken.removed);
      quotes = draft.quotes;
      text = draft.text;
      then();
      return true;
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
</script>

<ChatComposer bind:value={text} bind:quotes onRemoveQuote={removeQuote} {sendControl} />
