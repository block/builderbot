/**
 * The project a "New tag…" context-menu item asked to create a tag for.
 *
 * The menu closes (and unmounts its items) as the item is picked, so the
 * dialog can't live inside it: one NewTagDialog, mounted in App.svelte,
 * opens for whatever project was last requested here.
 */

import type { Project } from '../../types';

class NewTagDialogRequest {
  private _project = $state<Project | null>(null);
  private _open = $state(false);

  get project(): Project | null {
    return this._project;
  }

  get open(): boolean {
    return this._open;
  }

  set open(value: boolean) {
    this._open = value;
  }

  request(project: Project): void {
    this._project = project;
    this._open = true;
  }
}

export const newTagDialog = new NewTagDialogRequest();
