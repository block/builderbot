<!--
  ProjectCardMenuContent.svelte — a project card's context menu, shared by the
  home grid and the board so a card offers the same actions in either view:
  Mark as Unread, the project's tags, Transfer and Remove.

  On a board card, `moveToGroup` puts "Move to" for the board's group first
  (BoardMoveToMenu): on a board, changing column is the card's main action.
  That group's own submenu is then left out of the tags below, since it would
  offer the same choice under another name.
-->
<script lang="ts">
  import Mail from '@lucide/svelte/icons/mail';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project, TagGroup } from '../../types';
  import BoardMoveToMenu from '../board/BoardMoveToMenu.svelte';
  import { projectActions } from './projectActions.svelte';
  import ProjectTagMenuItems from './ProjectTagMenuItems.svelte';
  import ProjectTransferMenuItem from './ProjectTransferMenuItem.svelte';

  interface Props {
    project: Project;
    /** Mid-transfer, which the backend won't delete. */
    transferring: boolean;
    onRemove: () => void;
    /** On a board card: the board's group. */
    moveToGroup?: TagGroup | null;
    /** After a "Move to" pick that changed the project's column. */
    onMoved?: (projectId: string) => void;
  }

  let { project, transferring, onRemove, moveToGroup = null, onMoved }: Props = $props();
</script>

<ContextMenu.Content class="min-w-[172px]">
  {#if moveToGroup}
    <BoardMoveToMenu {project} group={moveToGroup} {onMoved} />
    <ContextMenu.Separator />
  {/if}
  <ContextMenu.Item onSelect={() => projectActions.markProjectUnread(project)}>
    <Mail size={14} /> Mark as Unread
  </ContextMenu.Item>
  <ProjectTagMenuItems {project} omitGroupId={moveToGroup?.id ?? null} />
  <ContextMenu.Separator />
  <ProjectTransferMenuItem {project} />
  <ContextMenu.Item variant="destructive" disabled={transferring} onSelect={onRemove}>
    <Trash2 size={14} /> Remove Project
  </ContextMenu.Item>
</ContextMenu.Content>
