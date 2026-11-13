/**
 * The actions the repo settings panel lists for its selected repo: read from
 * the backend when the selection changes, and written locally from the result
 * of each create, edit, delete and detection so the panel needs no reload.
 *
 * Both race. The backend serves reads concurrently, so two sent in a row can
 * answer in either order: picking repo A and then B could leave A's actions
 * listed under B, where Edit and Delete act on A's action ids. And a read
 * sent before a write landed may have read the store before it: applied
 * after the local write, it would put a deleted action back, and the panel
 * does not reload after a delete, so the row would stay and its Edit and
 * Delete fail with "Action not found".
 */

import type { ProjectAction } from '../../api/commands';
import { LatestWins } from '../../shared/latestWins';

/** The repo whose actions are listed. */
export interface RepoActionScope {
  githubRepo: string;
  subpath?: string | null;
}

export class RepoActionList {
  actions = $state<ProjectAction[]>([]);
  loading = $state(false);

  /** `read`: the read responses. `written`: the local writes. */
  private order = new LatestWins<'read' | 'written'>();

  /**
   * @param selected The repo to list, asked afresh for every read.
   * @param read Reads a repo's actions from the backend.
   */
  constructor(
    private readonly selected: () => RepoActionScope | null,
    private readonly read: (githubRepo: string, subpath?: string) => Promise<ProjectAction[]>
  ) {}

  /** Read the selected repo's actions. A failed read empties the list. */
  async load(): Promise<void> {
    const ticket = this.order.issue();
    const scope = this.selected();
    if (!scope) {
      // Nothing to read, but a read still in flight must not fill the list.
      this.order.admit('read', ticket);
      this.actions = [];
      return;
    }
    this.loading = true;
    try {
      let loaded: ProjectAction[];
      try {
        loaded = await this.read(scope.githubRepo, scope.subpath ?? undefined);
      } catch (e) {
        console.error('Failed to load actions:', e);
        loaded = [];
      }
      // A read sent after this one has already landed.
      if (!this.order.admit('read', ticket)) return;
      // A write landed after this read was sent, so the read may predate it.
      // It is not just dropped: it may be the read for a repo selected since,
      // whose list the write (to the rows still on screen) knows nothing of.
      // Read again, now that the write is in the store.
      if (this.order.overtaken('written', ticket)) return await this.load();
      this.actions = loaded;
    } finally {
      this.loading = false;
    }
  }

  /** Replace the list with the result of a write that has landed. */
  write(actions: ProjectAction[]): void {
    this.order.admit('written', this.order.issue());
    this.actions = actions;
  }
}
