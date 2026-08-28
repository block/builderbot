<!--
  WriteNoteModal.svelte — author or edit a user-written note

  The counterpart to NoteModal (which only renders an agent's note): here the
  user types the markdown themselves. Opens empty for a new note and prefilled
  when an existing written note is clicked in the timeline.

  There is no title field — the note's leading H1 is its title, matching how
  session notes are stored.
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import PencilLine from '@lucide/svelte/icons/pencil-line';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import Spinner from '../../shared/Spinner.svelte';
  import { viewport } from '../../shared/viewport.svelte';
  import MarkdownWysiwygEditor from './MarkdownWysiwygEditor.svelte';
  import { noteMarkdownWithTitle, splitNoteMarkdown } from './noteMarkdown';

  interface Props {
    open: boolean;
    /** Existing note to edit; omitted when writing a new one. */
    note?: { id: string; title: string; content: string } | null;
    /** Persist the note. Receives the title derived from the note's leading H1. */
    onSave: (note: { title: string; content: string }) => Promise<void>;
    onClose: () => void;
  }

  let { open, note = null, onSave, onClose }: Props = $props();

  let editor = $state<ReturnType<typeof MarkdownWysiwygEditor> | null>(null);
  let markdown = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);

  let isEdit = $derived(!!note);
  // Keyed so the editor remounts (and re-seeds its document) when the dialog
  // opens on a different note rather than reusing the previous one's content.
  let editorKey = $derived(open ? (note?.id ?? 'new') : null);
  let initialMarkdown = $derived(note ? noteMarkdownWithTitle(note.title, note.content) : '');
  // Button state only — `handleSave` re-checks against the editor rather than
  // this debounced copy of the document.
  let canSave = $derived(!saving && markdown.trim().length > 0);

  // Re-seed whenever the dialog opens (fresh or on a different note), so an
  // open never starts from a previous note's draft or saving state. Closing
  // is deliberately not reset: a successful save keeps `saving` set so the
  // footer doesn't flick back to "Save" mid close animation.
  $effect(() => {
    if (editorKey === null) return;
    markdown = initialMarkdown;
    saving = false;
    error = null;
  });

  async function handleSave() {
    // Deliberately not `canSave`: that reads the debounced `markdown` state,
    // so a note typed and saved by Cmd+Enter inside the debounce window would
    // silently no-op. The editor is the source of truth for both the emptiness
    // check and the content — markdownUpdated can lag the last keystroke.
    if (saving) return;
    const current = editor?.getMarkdown() ?? markdown;
    if (!current.trim()) return;
    const { title, body } = splitNoteMarkdown(current);

    saving = true;
    error = null;
    try {
      await onSave({ title, content: body });
      onClose();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not save the note.';
      saving = false;
    }
  }

  function requestClose() {
    if (saving) return;
    onClose();
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return;
    event.preventDefault();
    void handleSave();
  }
</script>

<Dialog.Root
  {open}
  onOpenChange={(next) => {
    if (!next) requestClose();
  }}
>
  <Dialog.Content
    class="h-[80vh] max-h-[900px] sm:max-w-[700px] p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
    onOpenAutoFocus={(e) => e.preventDefault()}
  >
    <Dialog.Header class="gap-0 border-b border-[var(--border-subtle)] p-0 flex-shrink-0">
      <div class="write-note-header">
        <span class="note-title-icon" aria-hidden="true">
          <PencilLine size={13} />
        </span>
        <Dialog.Title
          class="text-[var(--size-sm)] font-semibold text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
        >
          {isEdit ? 'Edit note' : 'Write note'}
        </Dialog.Title>
        <div class="header-actions">
          <Button
            variant="ghost"
            size="icon-sm"
            class="size-7 shrink-0 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
            title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
            aria-label="Close"
            onclick={requestClose}
          >
            <X size={16} />
          </Button>
        </div>
      </div>
    </Dialog.Header>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="editor-body" onkeydown={handleKeydown}>
      {#key editorKey}
        <MarkdownWysiwygEditor
          bind:this={editor}
          value={initialMarkdown}
          placeholder="Write your note — the first line becomes its title."
          onChange={(next) => (markdown = next)}
        />
      {/key}
    </div>

    <div class="write-note-footer">
      {#if error}
        <p class="write-note-error" role="alert">{error}</p>
      {/if}
      <Button type="button" variant="outline" onclick={requestClose} disabled={saving}>
        Cancel
      </Button>
      <!-- The spinner overlays an invisible label so the button keeps its
           width while saving and the Cancel button next to it never shifts. -->
      <Button
        type="button"
        class="relative"
        onclick={handleSave}
        disabled={!canSave}
        aria-busy={saving}
      >
        {#if saving}
          <span class="absolute inset-0 flex items-center justify-center">
            <Spinner size={14} />
          </span>
        {/if}
        <span class={saving ? 'invisible' : ''}>Save</span>
      </Button>
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  .write-note-header {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    width: 100%;
    padding: 12px 16px;
  }

  .note-title-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 5px;
    flex-shrink: 0;
    color: var(--note-color);
    background: var(--note-bg);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    margin-left: auto;
  }

  .editor-body {
    display: flex;
    flex: 1;
    min-height: 0;
    flex-direction: column;
    overflow: hidden;
  }

  .write-note-footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    flex-shrink: 0;
  }

  .write-note-error {
    margin: 0 auto 0 0;
    min-width: 0;
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }
</style>
