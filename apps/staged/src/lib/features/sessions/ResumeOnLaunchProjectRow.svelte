<!--
  ResumeOnLaunchProjectRow.svelte - one project in the launch resume dialog

  A checkbox with the project's name and, under it, its interrupted sessions
  (`rebase on drafts-out-of-sync`). The whole row is the checkbox's label, so
  a click anywhere on it toggles. A project with nothing that can be resumed
  is shown disabled and unchecked, each session with the reason it is not.
-->
<script lang="ts">
  import { Checkbox } from '$lib/components/ui/checkbox';
  import {
    dispositionNote,
    rowSelectable,
    type InterruptedProjectRow,
  } from './resumeOnLaunchGroups';

  let {
    row,
    checked,
    disabled = false,
    onCheckedChange,
  }: {
    row: InterruptedProjectRow;
    checked: boolean;
    /** The dialog is answering; every row is locked. */
    disabled?: boolean;
    onCheckedChange: (checked: boolean) => void;
  } = $props();

  const id = $props.id();
  const selectable = $derived(rowSelectable(row));
</script>

<label class="row" for={id} data-selectable={selectable} data-row-key={row.key}>
  <Checkbox
    {id}
    class="mt-px data-unchecked:bg-[var(--bg-primary)] dark:data-unchecked:bg-[var(--bg-primary)]"
    checked={selectable && checked}
    disabled={disabled || !selectable}
    aria-describedby="{id}-sessions"
    onCheckedChange={(value) => onCheckedChange(value === true)}
  />
  <span class="text">
    <span class="name">{row.name}</span>
    <span class="sessions" id="{id}-sessions">
      {#each row.sessions as session (session.sessionId)}
        {@const note = dispositionNote(session)}
        <span class="session" data-disposition={session.disposition}>
          {session.label}{#if note}<span class="note">{` (${note})`}</span>{/if}
        </span>
      {/each}
    </span>
  </span>
</label>

<style>
  .row {
    display: flex;
    align-items: flex-start;
    gap: 0.625rem;
    padding: 0.5rem 0.625rem;
    border-radius: 0.375rem;
    cursor: pointer;
    user-select: none;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .row[data-selectable='false'] {
    cursor: default;
  }

  .row[data-selectable='false']:hover {
    background: transparent;
  }

  .row[data-selectable='false'] .name {
    color: var(--text-secondary);
  }

  .text {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    min-width: 0;
  }

  .name {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
    overflow-wrap: anywhere;
  }

  .sessions {
    display: flex;
    flex-direction: column;
    font-size: 0.8125rem;
    color: var(--text-secondary);
  }

  .session {
    overflow-wrap: anywhere;
  }

  .note {
    color: var(--text-muted);
  }
</style>
