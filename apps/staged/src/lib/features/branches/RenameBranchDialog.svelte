<script lang="ts" module>
  let inputCounter = 0;
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import Spinner from '../../shared/Spinner.svelte';

  interface Props {
    open: boolean;
    branchName: string;
    onRename?: (branchName: string) => void | Promise<void>;
  }

  let { open = $bindable(false), branchName, onRename }: Props = $props();

  const inputId = `rename-branch-${++inputCounter}`;
  let draft = $state('');
  let error = $state<string | null>(null);
  let renaming = $state(false);
  let inputElement = $state<HTMLInputElement | null>(null);
  let wasOpen = false;

  $effect(() => {
    if (open && !wasOpen) {
      draft = branchName;
      error = null;
      renaming = false;
      void tick().then(() => {
        inputElement?.focus();
        inputElement?.select();
      });
    }
    wasOpen = open;
  });

  function errorMessage(e: unknown): string {
    if (e instanceof Error) return e.message;
    return String(e);
  }

  function requestClose() {
    if (renaming) return;
    open = false;
  }

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (renaming) return;

    const trimmed = draft.trim();
    if (!trimmed) {
      error = 'Enter a branch name.';
      return;
    }
    if (trimmed === branchName) {
      error = 'Use a different branch name.';
      return;
    }

    renaming = true;
    error = null;
    try {
      await onRename?.(trimmed);
      open = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      renaming = false;
    }
  }
</script>

<Dialog.Root
  {open}
  onOpenChange={(nextOpen) => {
    if (nextOpen) {
      open = true;
    } else {
      requestClose();
    }
  }}
>
  <Dialog.Content class="sm:max-w-[420px] gap-4">
    <Dialog.Header>
      <Dialog.Title>Rename Branch</Dialog.Title>
    </Dialog.Header>

    <form class="rename-form" onsubmit={handleSubmit}>
      <div class="rename-field">
        <Label for={inputId}>Branch name</Label>
        <Input
          id={inputId}
          bind:ref={inputElement}
          bind:value={draft}
          disabled={renaming}
          aria-invalid={error ? 'true' : undefined}
          oninput={() => (error = null)}
        />
      </div>

      {#if error}
        <p class="rename-error" role="alert">{error}</p>
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={requestClose} disabled={renaming}>
          Cancel
        </Button>
        <Button type="submit" disabled={renaming}>
          {#if renaming}
            <Spinner size={14} />
            <span>Renaming...</span>
          {:else}
            Rename
          {/if}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .rename-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .rename-field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .rename-error {
    margin: 0;
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }
</style>
