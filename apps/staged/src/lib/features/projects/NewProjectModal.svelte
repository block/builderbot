<!--
  NewProjectModal.svelte - Modal wrapper for project creation form

  Renders the NewProjectForm inside a dialog overlay with header and close button.
-->
<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import type { Project } from '../../types';
  import type { RepoSelection } from '../../shared/githubUrl';
  import NewProjectForm from './NewProjectForm.svelte';

  interface Props {
    open: boolean;
    onCreated: (project: Project) => void;
    onClose: () => void;
    /** Preselect this repo (and subpath) in the form. */
    initialRepo?: RepoSelection | null;
  }

  let { open, onCreated, onClose, initialRepo = null }: Props = $props();
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && onClose()}>
  <Dialog.Content class="sm:max-w-[460px]">
    <Dialog.Header>
      <Dialog.Title>New Project</Dialog.Title>
    </Dialog.Header>

    <NewProjectForm {onCreated} onCancel={onClose} {initialRepo} />
  </Dialog.Content>
</Dialog.Root>
