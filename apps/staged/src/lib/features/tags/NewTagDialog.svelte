<!--
  NewTagDialog.svelte — name a new tag for a project, from the "New tag…"
  item in its context menu (see newTagRequest.svelte.ts). The tag is created
  ungrouped with an automatic colour and assigned to the project in one
  write. A name an ungrouped tag already has just assigns that tag.

  Mounted once, in App.svelte.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import Spinner from '../../shared/Spinner.svelte';
  import { autofocusOnOpen } from '../../shared/focusAfterDialogEnter';
  import { tagsStore } from '../../stores/tags.svelte';
  import { newTagDialog } from './newTagRequest.svelte';
  import { tagActions } from './tagActions.svelte';
  import { MAX_TAG_NAME_CHARS, tagNameKey } from './tagState';

  const inputId = 'new-tag-name';

  let draft = $state('');
  let error = $state<string | null>(null);
  let creating = $state(false);
  let inputElement = $state<HTMLInputElement | null>(null);
  let wasOpen = false;

  $effect(() => {
    const open = newTagDialog.open;
    if (open && !wasOpen) {
      draft = '';
      error = null;
      creating = false;
      void tick().then(() => autofocusOnOpen(inputElement));
    }
    wasOpen = open;
  });

  function requestClose() {
    if (creating) return;
    newTagDialog.open = false;
  }

  async function handleSubmit(event: SubmitEvent) {
    event.preventDefault();
    const project = newTagDialog.project;
    if (creating || !project) return;

    const name = draft.trim();
    if (!name) {
      error = 'Enter a tag name.';
      return;
    }
    if ([...name].length > MAX_TAG_NAME_CHARS) {
      error = `Use at most ${MAX_TAG_NAME_CHARS} characters.`;
      return;
    }

    const existing = (tagsStore.tagsByGroup.get(null) ?? []).find(
      (tag) => tagNameKey(tag.name) === tagNameKey(name)
    );
    if (existing) {
      newTagDialog.open = false;
      void tagActions.setProjectTagAssigned(project, existing.id, true);
      return;
    }

    creating = true;
    error = null;
    const created = await tagActions.createTagForProject(project, name);
    creating = false;
    // A failure has already toasted; keep the dialog so the name can be fixed.
    if (created) newTagDialog.open = false;
  }
</script>

<Dialog.Root
  bind:open={
    () => newTagDialog.open, (next) => (next ? (newTagDialog.open = true) : requestClose())
  }
>
  <Dialog.Content class="sm:max-w-[380px] gap-4">
    <Dialog.Header>
      <Dialog.Title>New Tag</Dialog.Title>
    </Dialog.Header>

    <form class="new-tag-form" onsubmit={handleSubmit}>
      <div class="new-tag-field">
        <Label for={inputId}>Name</Label>
        <Input
          id={inputId}
          bind:ref={inputElement}
          bind:value={draft}
          disabled={creating}
          placeholder="e.g. q4-launch"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck="false"
          aria-invalid={error ? 'true' : undefined}
          oninput={() => (error = null)}
        />
      </div>

      {#if error}
        <p class="new-tag-error" role="alert">{error}</p>
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={requestClose} disabled={creating}>
          Cancel
        </Button>
        <Button type="submit" disabled={creating}>
          {#if creating}
            <Spinner size={14} />
            <span>Creating…</span>
          {:else}
            Create
          {/if}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .new-tag-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .new-tag-field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .new-tag-error {
    margin: 0;
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }
</style>
