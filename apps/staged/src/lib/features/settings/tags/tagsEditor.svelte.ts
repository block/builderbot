/**
 * Settings › Tags' state and actions, shared by the panel's sections and
 * rows so they don't thread a dozen callbacks through each other.
 *
 * Catalog writes go through tagActions, which queues them, awaits the
 * backend and then patches tagsStore. Tag moves (drag and the row menu) also
 * show at once: the editor's view of the catalog is the store's snapshot
 * with the moves still in flight applied on top, in order. Each move is
 * resolved against that view, so Move down pressed three times quickly
 * steps from where the last press left the tag, and leaves the list once its
 * write lands (the store then holds it). A failed move clears them all and
 * starts a new generation: the later ones were resolved against an order
 * that never happened, so the ones still queued are skipped, not sent.
 *
 * Create one per mounted panel; its $derived fields belong to that
 * component.
 */

import { tick } from 'svelte';
import { toast } from 'svelte-sonner';
import type { DeleteTagGroupMode, Tag, TagGroup } from '../../../types';
import { projectsDataStore } from '../../../stores/projectsData.svelte';
import { tagsStore } from '../../../stores/tags.svelte';
import { tagActions } from '../../tags/tagActions.svelte';
import { withTagMoved, type TagMove } from '../../tags/tagCatalogPatches';
import {
  resolveTagDrop,
  stepTarget,
  steppedGroupOrder,
  type ResolvedTagMove,
  type TagDropTarget,
} from '../../tags/tagReorder';
import { indexTagCatalog, numberedName, tagNameKey, type TagCatalog } from '../../tags/tagState';
import { freeGroupName, type TagGroupTemplate } from '../../tags/tagTemplates';

/** The name input of a tag row. */
export function tagNameInputId(tagId: string): string {
  return `tag-settings-name-${tagId}`;
}

/** A tag row's ⋯ menu trigger, refocused after a move. */
export function tagMenuTriggerId(tagId: string): string {
  return `tag-settings-menu-${tagId}`;
}

/** A group section's name input. */
export function groupNameInputId(groupId: string): string {
  return `tag-settings-group-name-${groupId}`;
}

/** A group section's ⋯ menu trigger, refocused after a reorder. */
export function groupMenuTriggerId(groupId: string): string {
  return `tag-settings-group-menu-${groupId}`;
}

/** How the ungrouped scope is named in menus and messages. */
export const UNGROUPED_LABEL = 'Ungrouped';

const NEW_TAG_NAME = 'New tag';

/** A move in flight. The token finds it again once its write settles. */
interface PendingMove {
  token: number;
  move: TagMove;
}

export class TagsEditor {
  /**
   * Moves whose writes are in flight, oldest first. Raw, and replaced
   * wholesale: a deep `$state` proxy would hand back proxies, not the objects
   * pushed in, and the list is only ever read as a whole.
   */
  private pendingMoves = $state.raw<readonly PendingMove[]>([]);
  /** Bumped when a move fails; moves queued under an older one are skipped. */
  private moveGeneration = 0;
  private nextMoveToken = 0;

  /** The store's snapshot with the pending moves applied on top. */
  private readonly view = $derived(
    this.pendingMoves.reduce((state, { move }) => withTagMoved(state, move), tagsStore.snapshot)
  );

  /** The catalog every section renders. */
  readonly catalog: TagCatalog = $derived(indexTagCatalog(this.view));

  /** The tag being dragged, if a drag started in this panel. */
  dragTagId = $state<string | null>(null);
  /** Where the dragged tag would land. */
  dropTarget = $state<TagDropTarget | null>(null);
  /** Whether dropping now would change anything; hides no-op indicators. */
  readonly dropWouldMove = $derived(
    this.dragTagId !== null &&
      this.dropTarget !== null &&
      resolveTagDrop(this.catalog, this.dragTagId, this.dropTarget) !== null
  );

  // Each dialog's target outlives its close, so its text doesn't blank
  // mid-animation.
  deleteTagOpen = $state(false);
  deleteTagTarget = $state<Tag | null>(null);
  deleteGroupOpen = $state(false);
  deleteGroupTarget = $state<TagGroup | null>(null);
  moveConfirmOpen = $state(false);
  moveConfirmTarget = $state<ResolvedTagMove | null>(null);

  creatingGroup = $state(false);
  private addingTagTo = $state<ReadonlySet<string | null>>(new Set());

  // ── Reads ──

  /** Live projects carrying the tag. */
  usage(tagId: string): number {
    return tagsStore.usageCount.get(tagId) ?? 0;
  }

  /** Live projects with a tag in the group (at most one each). */
  groupUsage(groupId: string): number {
    let total = 0;
    for (const tag of this.catalog.tagsByGroup.get(groupId) ?? []) total += this.usage(tag.id);
    return total;
  }

  /** A group's name, or "Ungrouped" for null. */
  scopeName(groupId: string | null): string {
    if (groupId === null) return UNGROUPED_LABEL;
    return this.catalog.groupsById.get(groupId)?.name ?? UNGROUPED_LABEL;
  }

  isAddingTag(groupId: string | null): boolean {
    return this.addingTagTo.has(groupId);
  }

  // ── Tags ──

  /** Append a "New tag" (numbered if taken) to a scope and focus its name. */
  async addTag(groupId: string | null): Promise<void> {
    if (this.addingTagTo.has(groupId)) return;
    this.addingTagTo = new Set(this.addingTagTo).add(groupId);
    try {
      const taken = new Set(
        (this.catalog.tagsByGroup.get(groupId) ?? []).map((t) => tagNameKey(t.name))
      );
      const tag = await tagActions.createTag({ name: numberedName(NEW_TAG_NAME, taken), groupId });
      if (tag) await focusAndSelect(tagNameInputId(tag.id));
    } finally {
      const next = new Set(this.addingTagTo);
      next.delete(groupId);
      this.addingTagTo = next;
    }
  }

  requestDeleteTag(tag: Tag): void {
    this.deleteTagTarget = tag;
    this.deleteTagOpen = true;
  }

  async confirmDeleteTag(): Promise<void> {
    // AlertDialog.Action doesn't close the dialog itself (only Cancel does).
    this.deleteTagOpen = false;
    if (this.deleteTagTarget) await tagActions.deleteTag(this.deleteTagTarget);
  }

  // ── Moving tags ──

  /**
   * Move a tag to `target`. A name the target scope already has is refused
   * up front; moving into a group where some projects already have a tag
   * asks first, since those projects lose this one.
   */
  requestMove(tagId: string, target: TagDropTarget, refocus = false): void {
    const projectTagIds = projectsDataStore.liveProjects.map((p) => tagsStore.tagIdsFor(p.id));
    const move = resolveTagDrop(this.catalog, tagId, target, projectTagIds);
    if (!move) return;
    if (move.collision) {
      const name = move.collision.name;
      toast.error(
        move.groupId === null
          ? `There's already an ungrouped tag named “${name}”`
          : `${this.scopeName(move.groupId)} already has a tag named “${name}”`,
        { description: 'Rename one of them first.' }
      );
      return;
    }
    if (move.conflicts > 0) {
      this.moveConfirmTarget = move;
      this.moveConfirmOpen = true;
      return;
    }
    void this.applyMove(move, refocus);
  }

  async confirmMove(): Promise<void> {
    this.moveConfirmOpen = false;
    if (this.moveConfirmTarget) await this.applyMove(this.moveConfirmTarget, true);
  }

  /** Move a tag one step up (-1) or down (1) in its scope. */
  moveStep(tagId: string, step: -1 | 1): void {
    const target = stepTarget(this.catalog, tagId, step);
    if (target) this.requestMove(tagId, target, true);
  }

  /** Move a tag to the end of another scope. */
  moveToScope(tagId: string, groupId: string | null): void {
    this.requestMove(tagId, { groupId, anchorTagId: null }, true);
  }

  private async applyMove(move: ResolvedTagMove, refocus: boolean): Promise<void> {
    const generation = this.moveGeneration;
    const pending: PendingMove = {
      token: this.nextMoveToken++,
      move: { tagId: move.tagId, groupId: move.groupId, index: move.index },
    };
    this.pendingMoves = [...this.pendingMoves, pending];
    // WebKit drops focus from a node a keyed {#each} moves; put it back on
    // the row's menu so Move up / Move down can repeat from the keyboard.
    if (refocus) void focusAfterRender(tagMenuTriggerId(move.tagId));
    // The bookkeeping runs inside tagActions' chain, so a failure is seen
    // here before the next queued move asks whether it's still wanted.
    await tagActions.moveTag(pending.move, {
      stillWanted: () => this.moveGeneration === generation,
      onSettled: (moved) => {
        // An earlier move of this generation failed: the overlay already
        // went, and this one was skipped.
        if (this.moveGeneration !== generation) return;
        if (moved) {
          // Once it lands the store's own patch holds the move, so the
          // overlay entry can go.
          this.pendingMoves = this.pendingMoves.filter((m) => m.token !== pending.token);
        } else {
          // The failure's refetch brings server truth. The later moves were
          // resolved against an order that never happened, so they go too,
          // and the ones still queued see the new generation and aren't sent.
          this.moveGeneration++;
          this.pendingMoves = [];
        }
      },
    });
  }

  // ── Drag and drop ──

  dragStart(tagId: string): void {
    this.dragTagId = tagId;
    this.dropTarget = null;
  }

  dragOver(target: TagDropTarget): void {
    if (this.dragTagId === null || sameTarget(this.dropTarget, target)) return;
    this.dropTarget = target;
  }

  drop(): void {
    const tagId = this.dragTagId;
    const target = this.dropTarget;
    this.dragEnd();
    if (tagId !== null && target !== null) this.requestMove(tagId, target);
  }

  dragEnd(): void {
    this.dragTagId = null;
    this.dropTarget = null;
  }

  /** Where the drop indicator sits on a row, if the current drop would land beside it. */
  dropPlaceFor(tagId: string): 'before' | 'after' | null {
    const target = this.dropTarget;
    if (!this.dropWouldMove || target?.anchorTagId !== tagId) return null;
    return target.place;
  }

  /** Whether the current drop would land at the end of the scope. */
  dropsAtEndOf(groupId: string | null): boolean {
    const target = this.dropTarget;
    return this.dropWouldMove && target?.anchorTagId === null && target.groupId === groupId;
  }

  // ── Groups ──

  /** Create a group from a template, then focus its name. */
  async createGroup(template: TagGroupTemplate): Promise<void> {
    if (this.creatingGroup) return;
    this.creatingGroup = true;
    try {
      const name = freeGroupName(template, this.catalog.groups);
      const group = await tagActions.createTagGroup(name, template.tags);
      if (group) await focusAndSelect(groupNameInputId(group.id));
    } finally {
      this.creatingGroup = false;
    }
  }

  renameGroup(group: TagGroup, name: string): Promise<boolean> {
    return tagActions.renameTagGroup(group, name);
  }

  /** Make the group drive the project icon, or turn that off. */
  setIconGroup(group: TagGroup, on: boolean): void {
    if (group.isIconGroup === on) return;
    void tagActions.setIconTagGroup(on ? group.id : null);
  }

  /** Pin the group's board to the projects sidebar, or unpin it. */
  setSidebarBoard(group: TagGroup, on: boolean): void {
    if (group.showInSidebar === on) return;
    void tagActions.setTagGroupSidebar(group, on);
  }

  async moveGroup(group: TagGroup, step: -1 | 1): Promise<void> {
    const order = steppedGroupOrder(this.catalog.groups, group.id, step);
    if (!order) return;
    if (await tagActions.reorderTagGroups(order)) {
      await focusAfterRender(groupMenuTriggerId(group.id));
    }
  }

  requestDeleteGroup(group: TagGroup): void {
    this.deleteGroupTarget = group;
    this.deleteGroupOpen = true;
  }

  async confirmDeleteGroup(mode: DeleteTagGroupMode): Promise<void> {
    this.deleteGroupOpen = false;
    if (this.deleteGroupTarget) await tagActions.deleteTagGroup(this.deleteGroupTarget, mode);
  }
}

function sameTarget(a: TagDropTarget | null, b: TagDropTarget): boolean {
  if (!a || a.groupId !== b.groupId || a.anchorTagId !== b.anchorTagId) return false;
  return a.anchorTagId === null || (b.anchorTagId !== null && a.place === b.place);
}

async function focusAndSelect(id: string): Promise<void> {
  await tick();
  const input = document.getElementById(id);
  if (!(input instanceof HTMLInputElement)) return;
  input.focus();
  input.select();
  input.scrollIntoView?.({ block: 'nearest' });
}

/**
 * Focus an element once the moves in flight have rendered, and once more on
 * the next frame in case a closing menu restored focus to a node that has
 * since moved.
 */
async function focusAfterRender(id: string): Promise<void> {
  await tick();
  document.getElementById(id)?.focus();
  requestAnimationFrame(() => {
    const el = document.getElementById(id);
    if (el && document.activeElement !== el) el.focus();
  });
}
