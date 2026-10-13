<!--
  NoteTransferMenu.svelte — the "…" menu in a note dialog's header, with
  "Move to project…" and "Copy to project…", and the picker they open.

  Owns the whole round trip so NoteModal and WriteNoteModal only have to say
  which note is on screen: it calls the branch-note or project-note command
  for `noteKind`, toasts a successful copy (the note stays open), and reports
  a successful move through `onMoved` so the host can close — the note no
  longer belongs where it was opened from.
-->
<script lang="ts">
  import Copy from '@lucide/svelte/icons/copy';
  import FolderInput from '@lucide/svelte/icons/folder-input';
  import MoreHorizontal from '@lucide/svelte/icons/more-horizontal';
  import { toast } from 'svelte-sonner';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import * as commands from '../../api/commands';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import NoteTransferDialog from './NoteTransferDialog.svelte';
  import type { NoteTransferMode } from './noteTransferTarget';

  interface Props {
    noteId: string;
    noteKind: 'branch' | 'project';
    noteTitle: string;
    /** The project the note is in now. */
    projectId?: string | null;
    /** The repo a branch note is bound to; shown in the picker's hint. */
    repoLabel?: { githubRepo: string; subpath: string | null } | null;
    /** The note left this project; the host should close. */
    onMoved?: () => void;
  }

  let {
    noteId,
    noteKind,
    noteTitle,
    projectId = null,
    repoLabel = null,
    onMoved,
  }: Props = $props();

  let mode = $state<NoteTransferMode>('move');
  let dialogOpen = $state(false);

  function openPicker(next: NoteTransferMode) {
    mode = next;
    dialogOpen = true;
  }

  function targetName(targetProjectId: string): string {
    return projectsDataStore.projects.find((p) => p.id === targetProjectId)?.name ?? 'the project';
  }

  async function transfer(targetProjectId: string) {
    if (mode === 'move') {
      if (noteKind === 'project') {
        await commands.moveProjectNoteToProject(noteId, targetProjectId);
      } else {
        await commands.moveNoteToProject(noteId, targetProjectId);
      }
      onMoved?.();
      return;
    }
    if (noteKind === 'project') {
      await commands.copyProjectNoteToProject(noteId, targetProjectId);
    } else {
      await commands.copyNoteToProject(noteId, targetProjectId);
    }
    toast.success(`Copied to ${targetName(targetProjectId)}`, {
      description: `A copy of ${noteTitle ? `"${noteTitle}"` : 'the note'} is now in that project.`,
    });
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-[var(--bg-hover)] hover:text-foreground focus-visible:bg-[var(--bg-hover)] focus-visible:text-foreground focus-visible:outline-none data-[state=open]:bg-[var(--bg-hover)] data-[state=open]:text-foreground"
    aria-label="More note actions"
    title="More actions"
  >
    <MoreHorizontal size={16} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[180px]">
    <DropdownMenu.Item onSelect={() => openPicker('move')}>
      <FolderInput size={14} />
      Move to project…
    </DropdownMenu.Item>
    <DropdownMenu.Item onSelect={() => openPicker('copy')}>
      <Copy size={14} />
      Copy to project…
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>

<NoteTransferDialog
  bind:open={dialogOpen}
  {mode}
  {noteTitle}
  currentProjectId={projectId}
  repoLabel={noteKind === 'branch' ? repoLabel : null}
  onConfirm={transfer}
/>
