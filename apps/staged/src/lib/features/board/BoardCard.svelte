<!--
  BoardCard.svelte — a project on the board: its status icon and name, the
  activity indicator (running, unread, …), and its repos with chips for the
  tags its column and icon don't already show. A column stands for the card's
  tag in the board's group, so that group's chip is the one left out; a card
  under "No ‹group›" has no such tag and shows every chip. Clicking opens the
  project; the context menu is the grid card's, plus "Move to" for the
  board's group.

  With drag and drop on (a fine pointer, desktop layout) the card drags to
  another column. A search hides the card rather than unmounting it.
-->
<script module lang="ts">
  import type { Project } from '../../types';

  /** What every card on a board shares, from ProjectsBoard. */
  export interface BoardCardHandlers {
    /** Cards drag between columns. */
    readonly dragEnabled: boolean;
    /** The card being dragged, if any. */
    readonly draggingId: string | null;
    open(projectId: string): void;
    startDrag(projectId: string, event: DragEvent): void;
    endDrag(): void;
    remove(project: Project): void;
    /** A "Move to" pick changed the card's column. */
    moved(projectId: string): void;
  }
</script>

<script lang="ts">
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { TagGroup } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectDisplayName } from '../../shared/utils';
  import { getProjectStatus } from '../projects/projectStatus';
  import { workspaceLifecycle } from '../projects/workspaceLifecycle.svelte';
  import ProjectCardMenuContent from '../projects/ProjectCardMenuContent.svelte';
  import ProjectCardStatusIndicator from '../projects/ProjectCardStatusIndicator.svelte';
  import ProjectCardLabels from '../projects/ProjectCardLabels.svelte';
  import ProjectStatusIcon from '../projects/ProjectStatusIcon.svelte';
  import RemoteProjectIcon from '../projects/RemoteProjectIcon.svelte';

  interface Props {
    project: Project;
    group: TagGroup;
    /** False while a search rules the card out. */
    visible: boolean;
    handlers: BoardCardHandlers;
  }

  let { project, group, visible, handlers }: Props = $props();

  let branches = $derived(projectsDataStore.branchesByProject.get(project.id) ?? []);
  let status = $derived(
    getProjectStatus(
      project.id,
      projectsDataStore.deletingProjectNames,
      branches,
      workspaceLifecycle.getWorktreeErrors(),
      projectsDataStore.transferringProjects
    )
  );
</script>

<div class="board-card-wrapper" hidden={!visible}>
  <ContextMenu.Root>
    <ContextMenu.Trigger class="contents">
      <button
        type="button"
        class="board-card"
        class:dragging={handlers.draggingId === project.id}
        data-board-card={project.id}
        draggable={handlers.dragEnabled}
        ondragstart={(event) => handlers.startDrag(project.id, event)}
        ondragend={() => handlers.endDrag()}
        onclick={() => handlers.open(project.id)}
      >
        <ProjectCardStatusIndicator projectId={project.id} {status} />
        <div class="board-card-header">
          <ProjectStatusIcon
            {project}
            {branches}
            hydrated={projectsDataStore.isProjectHydrated(project.id)}
            size={14}
          />
          <span class="board-card-name">{projectDisplayName(project)}</span>
        </div>
        <div class="board-card-footer">
          {#if project.location === 'remote'}
            <RemoteProjectIcon size={12} />
          {/if}
          <ProjectCardLabels
            {project}
            repos={projectsDataStore.reposByProject.get(project.id) ?? []}
            excludeTagGroupId={group.id}
          />
        </div>
      </button>
    </ContextMenu.Trigger>
    <ProjectCardMenuContent
      {project}
      transferring={status.kind === 'transferring'}
      onRemove={() => handlers.remove(project)}
      moveToGroup={group}
      onMoved={(projectId) => handlers.moved(projectId)}
    />
  </ContextMenu.Root>
</div>

<style>
  .board-card-wrapper {
    display: flex;
    flex-direction: column;
  }

  /* The display above outranks the UA's [hidden] rule. */
  .board-card-wrapper[hidden] {
    display: none;
  }

  .board-card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 6px;
    text-align: left;
    background: var(--bg-primary);
    border: none;
    border-radius: 8px;
    padding: 10px 12px;
    color: inherit;
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      opacity 0.15s ease;
    /* As the grid's cards: skip style, layout and paint for cards out of
       view (below the fold, or in a column scrolled off to the side). On the
       button, so its paint containment doesn't clip the focus ring. */
    content-visibility: auto;
    contain-intrinsic-size: auto 46px;
  }

  .board-card:hover {
    background-color: var(--bg-hover);
  }

  .board-card.dragging {
    opacity: 0.4;
  }

  .board-card-header {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    padding-right: 18px;
    color: var(--text-primary);
    font-weight: 600;
  }

  .board-card-header :global(svg) {
    flex-shrink: 0;
  }

  .board-card-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .board-card-footer {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .board-card-footer :global(.card-labels) {
    font-size: var(--size-xs);
  }

  .board-card :global(.status-indicator) {
    top: 12px;
    right: 10px;
  }
</style>
