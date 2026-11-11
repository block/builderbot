<!--
  ProjectCardMenuContent.svelte — a project card's context menu, shared by the
  home grid and the board so a card offers the same actions in either view:
  Mark as Unread, the project's tags, Transfer and Remove.

  On a board card, `moveToGroupId` adds "Move to" for the board's group (see
  ProjectTagMenuItems).
-->
<script lang="ts">
  import Mail from '@lucide/svelte/icons/mail';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project } from '../../types';
  import { projectActions } from './projectActions.svelte';
  import ProjectTagMenuItems from './ProjectTagMenuItems.svelte';
  import ProjectTransferMenuItem from './ProjectTransferMenuItem.svelte';

  interface Props {
    project: Project;
    /** Mid-transfer, which the backend won't delete. */
    transferring: boolean;
    onRemove: () => void;
    moveToGroupId?: string | null;
    onMoved?: (projectId: string) => void;
  }

  let { project, transferring, onRemove, moveToGroupId = null, onMoved }: Props = $props();
</script>

<ContextMenu.Content class="min-w-[172px]">
  <ContextMenu.Item onSelect={() => projectActions.markProjectUnread(project)}>
    <Mail size={14} /> Mark as Unread
  </ContextMenu.Item>
  <ProjectTagMenuItems {project} {moveToGroupId} {onMoved} />
  <ContextMenu.Separator />
  <ProjectTransferMenuItem {project} />
  <ContextMenu.Item variant="destructive" disabled={transferring} onSelect={onRemove}>
    <Trash2 size={14} /> Remove Project
  </ContextMenu.Item>
</ContextMenu.Content>
