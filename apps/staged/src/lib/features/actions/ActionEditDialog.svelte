<!--
  ActionEditDialog.svelte — add or edit one repo action in a modal.

  The settings panel owns the persisted list; this dialog only owns the draft.
  It seeds the draft from `action` (or the add-mode defaults) each time it
  opens, hands the values back through `onSave`, and stays open showing the
  error if that save rejects, so a failed write never drops the user's input.
-->
<script lang="ts" module>
  import { ACTION_TYPES, type ActionType } from './actions';

  /** The editable fields of an action, as the dialog hands them back. */
  export type ActionFormValues = {
    name: string;
    command: string;
    actionType: ActionType;
    pinned: boolean;
    icon: string | null;
  };

  let inputCounter = 0;
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import Save from '@lucide/svelte/icons/save';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import * as Select from '$lib/components/ui/select';
  import Spinner from '../../shared/Spinner.svelte';
  import type { ProjectAction } from '../../api/commands';
  import IconPicker from './IconPicker.svelte';

  interface Props {
    open: boolean;
    /** The action being edited, or null to add a new one. */
    action: ProjectAction | null;
    /** Whether a newly added action starts pinned — see shouldPinNewAction. */
    defaultPinned?: boolean;
    onSave: (values: ActionFormValues) => void | Promise<void>;
  }

  let { open = $bindable(false), action, defaultPinned = false, onSave }: Props = $props();

  const idBase = `action-edit-${++inputCounter}`;
  const nameId = `${idBase}-name`;
  const commandId = `${idBase}-command`;
  const typeId = `${idBase}-type`;
  const pinnedId = `${idBase}-pinned`;

  let draft = $state<ActionFormValues>({
    name: '',
    command: '',
    actionType: 'run',
    pinned: false,
    icon: null,
  });
  let error = $state<string | null>(null);
  let saving = $state(false);
  let nameElement = $state<HTMLInputElement | null>(null);
  let wasOpen = false;

  $effect(() => {
    if (open && !wasOpen) {
      draft = action
        ? {
            name: action.name,
            command: action.command,
            actionType: action.actionType,
            pinned: action.pinned,
            icon: action.icon,
          }
        : { name: '', command: '', actionType: 'run', pinned: defaultPinned, icon: null };
      error = null;
      saving = false;
      void tick().then(() => nameElement?.focus());
    }
    wasOpen = open;
  });

  let canSave = $derived(draft.name.trim().length > 0 && draft.command.trim().length > 0);

  function errorMessage(e: unknown): string {
    if (e instanceof Error) return e.message;
    return String(e);
  }

  function requestClose() {
    if (saving) return;
    open = false;
  }

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (saving || !canSave) return;

    saving = true;
    error = null;
    const submitted: ActionFormValues = {
      ...draft,
      name: draft.name.trim(),
      command: draft.command.trim(),
    };
    try {
      await onSave(submitted);
      open = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      saving = false;
    }
  }
</script>

<Dialog.Root bind:open={() => open, (nextOpen) => (nextOpen ? (open = true) : requestClose())}>
  <Dialog.Content class="sm:max-w-[460px] gap-4">
    <Dialog.Header>
      <Dialog.Title>{action ? 'Edit Action' : 'Add Action'}</Dialog.Title>
    </Dialog.Header>

    <form class="action-form" onsubmit={handleSubmit}>
      <div class="field">
        <Label for={nameId}>Name</Label>
        <div class="name-row">
          <IconPicker
            icon={draft.icon}
            actionType={draft.actionType}
            onSelect={(icon) => (draft.icon = icon)}
          />
          <Input
            id={nameId}
            class="bg-[var(--bg-primary)] dark:bg-[var(--bg-primary)]"
            bind:ref={nameElement}
            bind:value={draft.name}
            placeholder="Action name"
            disabled={saving}
          />
        </div>
      </div>

      <div class="field">
        <Label for={commandId}>Command</Label>
        <Input
          id={commandId}
          class="bg-[var(--bg-primary)] dark:bg-[var(--bg-primary)]"
          bind:value={draft.command}
          placeholder="Command"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck="false"
          disabled={saving}
        />
      </div>

      <div class="field">
        <Label for={typeId}>Type</Label>
        <Select.Root
          type="single"
          value={draft.actionType}
          onValueChange={(v) => (draft.actionType = v as ActionType)}
          disabled={saving}
        >
          <Select.Trigger
            id={typeId}
            class="w-full bg-[var(--bg-primary)] dark:bg-[var(--bg-primary)] dark:hover:bg-[var(--bg-primary)]"
          >
            {draft.actionType}
          </Select.Trigger>
          <Select.Content>
            {#each ACTION_TYPES as t (t)}
              <Select.Item value={t} label={t}>{t}</Select.Item>
            {/each}
          </Select.Content>
        </Select.Root>
      </div>

      <div class="pinned-row">
        <Checkbox
          id={pinnedId}
          class="data-unchecked:bg-[var(--bg-primary)] dark:data-unchecked:bg-[var(--bg-primary)]"
          bind:checked={draft.pinned}
          disabled={saving}
        />
        <Label for={pinnedId} class="text-muted-foreground font-normal">Show in card header</Label>
      </div>

      {#if error}
        <p class="form-error" role="alert">{error}</p>
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={requestClose} disabled={saving}>
          Cancel
        </Button>
        <Button type="submit" disabled={saving || !canSave}>
          {#if saving}
            <Spinner size={14} />
            <span>Saving...</span>
          {:else}
            <Save size={14} />
            Save
          {/if}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .action-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .name-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .pinned-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .form-error {
    margin: 0;
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }
</style>
