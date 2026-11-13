/**
 * Tag writes: which tags a project carries (optimistic), and the catalog
 * edits behind the settings editor (server-confirmed).
 *
 * Assignment writes target a slot (see TagSlot): a group's single choice, or
 * one ungrouped tag. Every surface repaints as soon as a pick is made, from
 * the intent tagsStore layers over its snapshot. Writes to different slots
 * commute in the database, so nothing here holds one back for another;
 * writes to one slot are chained per project, so the backend sees them in
 * click order. A failed write drops its intent, if no newer pick in the slot
 * has replaced it, and the view falls back to server truth.
 *
 * Catalog writes are rare, so they await the backend and then patch the
 * snapshot with the result; a failure toasts and refetches. They run one at
 * a time, in call order, so two quick edits reach the backend in the order
 * they were made and each sees the snapshot the one before it left.
 *
 * The two queues are not ordered against each other here. The command
 * wrappers send every tag write in call order, which is what keeps an
 * assignment still in flight ahead of a catalog edit to its tag or group.
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type {
  DeleteTagGroupMode,
  NewTag,
  Project,
  Tag,
  TagColor,
  TagGroup,
  TagState,
} from '../../types';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { tagsStore } from '../../stores/tags.svelte';
import {
  withGroup,
  withGroupName,
  withGroupOrder,
  withGroupSidebar,
  withIconGroup,
  withTag,
  withTagMoved,
  withTagValue,
  withoutGroup,
  withoutTag,
  type TagMove,
} from './tagCatalogPatches';
import { intentSlot, type TagIntent } from './tagState';

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function noop(): void {}

/** The value `updateTag` replaces a tag's with. */
export interface TagValue {
  name: string;
  color: TagColor;
  icon: string | null;
}

/**
 * How a caller that queues several catalog writes follows them. Both run
 * inside the chain, so `onSettled` for one write always runs before
 * `stillWanted` for the next, whatever order the callers' own awaits resume
 * in. Neither may throw.
 */
export interface CatalogWriteHooks {
  /**
   * Asked once the writes ahead have settled, just before this one is
   * issued. False skips it quietly: nothing is sent or patched, no toast,
   * and the write resolves false.
   */
  stillWanted?: () => boolean;
  /** Told whether the write landed (false for a failure or a skip). */
  onSettled?: (ok: boolean) => void;
}

class TagActionsController {
  /** Per project and slot, the tail of its write chain. */
  private readonly slotTails = new Map<string, Promise<void>>();
  /** The tail of the catalog write chain. */
  private catalogTail: Promise<void> = Promise.resolve();

  /**
   * Set a project's tag in a group, or clear it with null. No-op when the
   * project already shows that value (pending picks included) or is being
   * deleted.
   */
  async setProjectGroupTag(project: Project, groupId: string, tagId: string | null): Promise<void> {
    if (projectsDataStore.isProjectDeleting(project.id)) return;
    if ((tagsStore.groupTagFor(project.id, groupId)?.id ?? null) === tagId) return;
    await this.writeAssignment(project.id, { kind: 'group', groupId, tagId }, () =>
      commands.setProjectGroupTag(project.id, groupId, tagId)
    );
  }

  /**
   * Add or remove an ungrouped tag. A grouped tag is routed through its
   * group's slot instead, so picks in one group always queue together.
   */
  async setProjectTagAssigned(project: Project, tagId: string, assigned: boolean): Promise<void> {
    const groupId = tagsStore.tagsById.get(tagId)?.groupId ?? null;
    if (groupId !== null) {
      const current = tagsStore.groupTagFor(project.id, groupId)?.id ?? null;
      if (assigned) return this.setProjectGroupTag(project, groupId, tagId);
      if (current === tagId) return this.setProjectGroupTag(project, groupId, null);
      return;
    }
    if (projectsDataStore.isProjectDeleting(project.id)) return;
    if (tagsStore.tagIdsFor(project.id).has(tagId) === assigned) return;
    await this.writeAssignment(project.id, { kind: 'tag', tagId, assigned }, () =>
      commands.setProjectTagAssigned(project.id, tagId, assigned)
    );
  }

  /**
   * Create a tag and assign it to a project in one backend write: an
   * automatic colour, in `groupId` (replacing the project's tag there) or
   * ungrouped. A grouped create queues behind earlier picks in that group.
   * Resolves to the new tag, or null when it failed (already toasted).
   */
  async createTagForProject(
    project: Project,
    name: string,
    groupId: string | null = null
  ): Promise<Tag | null> {
    if (projectsDataStore.isProjectDeleting(project.id)) return null;
    let created: Tag | null = null;
    const create = async () => {
      let tag: Tag;
      try {
        tag = await commands.createTag({ name, groupId, assignProjectId: project.id });
      } catch (e) {
        toast.error('Unable to create tag', { description: errorMessage(e) });
        void tagsStore.requestRefresh();
        return;
      }
      created = tag;
      tagsStore.patchLocal((state) => withTag(state, tag));
      // Shown through a committed intent, which also supersedes any older
      // pick in the group, until a snapshot requested from here on lands.
      const intent: TagIntent =
        tag.groupId === null
          ? { kind: 'tag', tagId: tag.id, assigned: true }
          : { kind: 'group', groupId: tag.groupId, tagId: tag.id };
      const { slot, token } = tagsStore.beginIntent(project.id, intent);
      tagsStore.confirmIntent(project.id, slot, token);
    };
    if (groupId === null) await create();
    else await this.enqueue(project.id, `g:${groupId}`, create);
    return created;
  }

  // ── Catalog ──

  /** Append a tag to a group (or ungrouped). Resolves to it, or null on failure. */
  async createTag(input: {
    name: string;
    groupId: string | null;
    color?: TagColor | null;
    icon?: string | null;
  }): Promise<Tag | null> {
    let tag: Tag | null = null;
    await this.catalogWrite(
      'Unable to create tag',
      async () => {
        tag = await commands.createTag(input);
      },
      (state) => (tag ? withTag(state, tag) : state)
    );
    return tag;
  }

  /**
   * Change some of a tag's name, colour and icon. The rest is taken from the
   * tag as the store has it once the catalog writes ahead of this one have
   * settled, so a rename followed at once by a colour pick keeps both: the
   * second write is built on the first's result, not on the row's stale
   * props. Resolves to whether it saved; a tag that has gone meanwhile
   * fails with a toast.
   */
  editTag(tagId: string, patch: Partial<TagValue>): Promise<boolean> {
    let value: TagValue | null = null;
    return this.catalogWrite(
      'Unable to update tag',
      async () => {
        const tag = tagsStore.tagsById.get(tagId);
        if (!tag) throw new Error('The tag no longer exists.');
        value = { name: tag.name, color: tag.color, icon: tag.icon, ...patch };
        await commands.updateTag(tagId, value);
      },
      (state) => (value ? withTagValue(state, tagId, value) : state)
    );
  }

  /**
   * Move a tag within its scope or into another (see `move_tag`). Moving
   * into a group drops it from projects that already have a tag there.
   * Resolves to whether it moved.
   *
   * A move's index means something only against the order the moves before
   * it produced, so a caller queuing several can pass `hooks`: `onSettled`
   * tells it each outcome inside the chain, and `stillWanted` lets it skip
   * the moves it queued behind one that failed (see CatalogWriteHooks).
   */
  moveTag(move: TagMove, hooks?: CatalogWriteHooks): Promise<boolean> {
    return this.catalogWrite(
      'Unable to move tag',
      () => commands.moveTag(move.tagId, move.groupId, move.index),
      (state) => withTagMoved(state, move),
      hooks
    );
  }

  /** Delete a tag; projects carrying it lose it. Resolves to whether it went. */
  deleteTag(tag: Tag): Promise<boolean> {
    return this.catalogWrite(
      'Unable to delete tag',
      () => commands.deleteTag(tag.id),
      (state) => withoutTag(state, tag.id)
    );
  }

  /**
   * Create a group, appended last, with `tags` in order. Resolves to the
   * group, or null on failure. The command returns only the group, so its
   * tags show once the refetch that follows lands.
   */
  async createTagGroup(name: string, tags: readonly NewTag[] = []): Promise<TagGroup | null> {
    let group: TagGroup | null = null;
    await this.catalogWrite(
      'Unable to create group',
      async () => {
        group = await commands.createTagGroup(name, [...tags]);
      },
      (state) => (group ? withGroup(state, group) : state)
    );
    return group;
  }

  renameTagGroup(group: TagGroup, name: string): Promise<boolean> {
    return this.catalogWrite(
      'Unable to rename group',
      () => commands.renameTagGroup(group.id, name),
      (state) => withGroupName(state, group.id, name)
    );
  }

  /** Make a group drive the project icon, or turn that off with null. */
  setIconTagGroup(groupId: string | null): Promise<boolean> {
    return this.catalogWrite(
      'Unable to change the project icon group',
      () => commands.setIconTagGroup(groupId),
      (state) => withIconGroup(state, groupId)
    );
  }

  /** Pin a group's board to the projects sidebar, or unpin it. */
  setTagGroupSidebar(group: TagGroup, shown: boolean): Promise<boolean> {
    return this.catalogWrite(
      'Unable to change the sidebar boards',
      () => commands.setTagGroupSidebar(group.id, shown),
      (state) => withGroupSidebar(state, group.id, shown)
    );
  }

  /** Reorder the groups; `groupIds` must list every group once. */
  reorderTagGroups(groupIds: string[]): Promise<boolean> {
    return this.catalogWrite(
      'Unable to reorder groups',
      () => commands.reorderTagGroups(groupIds),
      (state) => withGroupOrder(state, groupIds)
    );
  }

  /** Delete a group, keeping its tags as ungrouped ones or deleting them too. */
  deleteTagGroup(group: TagGroup, mode: DeleteTagGroupMode): Promise<boolean> {
    return this.catalogWrite(
      'Unable to delete group',
      () => commands.deleteTagGroup(group.id, mode),
      (state) => withoutGroup(state, group.id, mode)
    );
  }

  /**
   * Queue a catalog write behind the earlier ones, await it, then patch the
   * snapshot with what it did. A failure toasts and refetches. Resolves to
   * whether it succeeded; never rejects.
   *
   * The chain's tail swallows everything, including anything the step could
   * throw outside its try blocks, so one bad step can't leave every later
   * catalog write waiting on a rejected promise. A patch that throws (the
   * write itself landed) is logged and the refetch brings the truth.
   */
  private catalogWrite(
    failure: string,
    write: () => Promise<void>,
    patch: (state: TagState) => TagState,
    hooks: CatalogWriteHooks = {}
  ): Promise<boolean> {
    const run = this.catalogTail.then(async () => {
      const ok = await this.performCatalogWrite(failure, write, patch, hooks);
      hooks.onSettled?.(ok);
      return ok;
    });
    this.catalogTail = run.then(noop, noop);
    return run;
  }

  /** One step of the catalog chain: skip, or write and patch. */
  private async performCatalogWrite(
    failure: string,
    write: () => Promise<void>,
    patch: (state: TagState) => TagState,
    hooks: CatalogWriteHooks
  ): Promise<boolean> {
    if (hooks.stillWanted && !hooks.stillWanted()) return false;
    try {
      await write();
    } catch (e) {
      toast.error(failure, { description: errorMessage(e) });
      void tagsStore.requestRefresh();
      return false;
    }
    try {
      tagsStore.patchLocal(patch);
    } catch (e) {
      console.error('Failed to apply a tag catalog patch locally; refetching', e);
      void tagsStore.requestRefresh();
    }
    return true;
  }

  // ── Plumbing ──

  private writeAssignment(
    projectId: string,
    intent: TagIntent,
    write: () => Promise<void>
  ): Promise<void> {
    const { slot, token } = tagsStore.beginIntent(projectId, intent);
    return this.enqueue(projectId, intentSlot(intent), async () => {
      try {
        await write();
        tagsStore.confirmIntent(projectId, slot, token);
      } catch (e) {
        tagsStore.dropIntent(projectId, slot, token);
        toast.error('Unable to update tags', { description: errorMessage(e) });
      }
    });
  }

  /** Run `task` after every earlier task in the project's slot. `task` must not reject. */
  private enqueue(projectId: string, slot: string, task: () => Promise<void>): Promise<void> {
    const key = `${projectId}\u0000${slot}`;
    const run = (this.slotTails.get(key) ?? Promise.resolve()).then(task);
    this.slotTails.set(key, run);
    void run.then(() => {
      if (this.slotTails.get(key) === run) this.slotTails.delete(key);
    });
    return run;
  }
}

export const tagActions = new TagActionsController();
