/**
 * Tag writes: which tags a project carries (optimistic), and the catalog
 * edits behind the settings editor (server-confirmed).
 *
 * Assignment writes target a slot (see TagSlot): a group's single choice, or
 * one ungrouped tag. Every surface repaints as soon as a pick is made, from
 * the intent tagsStore layers over its snapshot. Writes to different slots
 * commute in the database, so they run concurrently; writes to one slot are
 * chained per project, so the backend sees them in click order. A failed
 * write drops its intent, if no newer pick in the slot has replaced it, and
 * the view falls back to server truth.
 *
 * Catalog writes are rare, so they await the backend and then patch the
 * snapshot with the result; a failure toasts and refetches.
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type { Project, Tag, TagColor, TagState } from '../../types';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { tagsStore } from '../../stores/tags.svelte';
import { intentSlot, type TagIntent } from './tagState';

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** The value `updateTag` replaces a tag's with. */
export interface TagValue {
  name: string;
  color: TagColor;
  icon: string | null;
}

class TagActionsController {
  /** Per project and slot, the tail of its write chain. */
  private readonly slotTails = new Map<string, Promise<void>>();

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
      tagsStore.patchLocal((state) => insertTag(state, tag));
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
    try {
      const tag = await commands.createTag(input);
      tagsStore.patchLocal((state) => insertTag(state, tag));
      return tag;
    } catch (e) {
      toast.error('Unable to create tag', { description: errorMessage(e) });
      void tagsStore.requestRefresh();
      return null;
    }
  }

  /** Replace a tag's name, colour and icon. Resolves to whether it saved. */
  async updateTag(tag: Tag, value: TagValue): Promise<boolean> {
    try {
      await commands.updateTag(tag.id, value);
    } catch (e) {
      toast.error('Unable to update tag', { description: errorMessage(e) });
      void tagsStore.requestRefresh();
      return false;
    }
    const next = { ...value, name: value.name.trim() };
    tagsStore.patchLocal((state) => ({
      ...state,
      tags: state.tags.map((t) => (t.id === tag.id ? { ...t, ...next } : t)),
    }));
    return true;
  }

  /** Delete a tag; projects carrying it lose it. Resolves to whether it went. */
  async deleteTag(tag: Tag): Promise<boolean> {
    try {
      await commands.deleteTag(tag.id);
    } catch (e) {
      toast.error('Unable to delete tag', { description: errorMessage(e) });
      void tagsStore.requestRefresh();
      return false;
    }
    tagsStore.patchLocal((state) => ({
      ...state,
      tags: state.tags.filter((t) => t.id !== tag.id),
      assignments: state.assignments.filter((a) => a.tagId !== tag.id),
    }));
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

/** Add a just-created tag to the snapshot, unless a refetch already has it. */
function insertTag(state: TagState, tag: Tag): TagState {
  if (state.tags.some((t) => t.id === tag.id)) return state;
  return { ...state, tags: [...state.tags, tag] };
}

export const tagActions = new TagActionsController();
