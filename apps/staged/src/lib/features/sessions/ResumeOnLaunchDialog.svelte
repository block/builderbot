<!--
  ResumeOnLaunchDialog.svelte - "Resume interrupted sessions?" at launch

  Mounted once in App.svelte (desktop main window only) and driven by
  resumeOnLaunch.pending. Lists one row per project with a checkbox; every
  project that can be resumed starts checked. Resume restarts the checked
  projects' sessions, Dismiss none; either way the rest are left as they are
  and not offered again. Closing the dialog any other way counts as Dismiss,
  so the offer is always recorded as handled.

  The title, summary and buttons stay put; only the list scrolls, so the
  buttons are reachable however many projects there are.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { resumeOnLaunch } from './resumeOnLaunch.svelte';
  import ResumeOnLaunchProjectRow from './ResumeOnLaunchProjectRow.svelte';
  import { interruptedSummary, rowSelectable } from './resumeOnLaunchGroups';

  const selectAllId = $props.id();

  const rows = $derived(resumeOnLaunch.rows);
  const summary = $derived(
    resumeOnLaunch.pending ? interruptedSummary(resumeOnLaunch.pending) : ''
  );
  const selectableCount = $derived(rows.filter(rowSelectable).length);
  const selectedCount = $derived(
    rows.filter((row) => rowSelectable(row) && resumeOnLaunch.isSelected(row.key)).length
  );
  // Only worth a header when there is more than one row to choose between.
  const showSelectAll = $derived(selectableCount > 1);
</script>

<AlertDialog.Root
  open={resumeOnLaunch.pending !== null}
  onOpenChange={(open) => {
    if (!open && resumeOnLaunch.pending && !resumeOnLaunch.busy) void resumeOnLaunch.dismiss();
  }}
>
  <AlertDialog.Content class="flex max-h-[min(85dvh,42rem)] flex-col">
    {#if resumeOnLaunch.pending}
      <AlertDialog.Header class="shrink-0">
        <AlertDialog.Title>Resume interrupted sessions?</AlertDialog.Title>
        <AlertDialog.Description>{summary}</AlertDialog.Description>
      </AlertDialog.Header>
      <div class="list">
        {#if showSelectAll}
          <div class="list-header">
            <label class="select-all" for={selectAllId}>
              <Checkbox
                id={selectAllId}
                class="data-unchecked:bg-[var(--bg-primary)] dark:data-unchecked:bg-[var(--bg-primary)]"
                checked={selectedCount === selectableCount}
                indeterminate={selectedCount > 0 && selectedCount < selectableCount}
                disabled={resumeOnLaunch.busy}
                onCheckedChange={(checked) => resumeOnLaunch.setAllSelected(checked === true)}
              />
              <span>Select all</span>
            </label>
            <span class="count">{selectedCount} of {selectableCount} selected</span>
          </div>
        {/if}
        <ul class="rows" aria-label="Interrupted projects">
          {#each rows as row (row.key)}
            <li>
              <ResumeOnLaunchProjectRow
                {row}
                checked={resumeOnLaunch.isSelected(row.key)}
                disabled={resumeOnLaunch.busy}
                onCheckedChange={(checked) => resumeOnLaunch.setSelected(row.key, checked)}
              />
            </li>
          {/each}
        </ul>
      </div>
      <AlertDialog.Footer class="shrink-0">
        <AlertDialog.Cancel disabled={resumeOnLaunch.busy} onclick={() => resumeOnLaunch.dismiss()}>
          Dismiss
        </AlertDialog.Cancel>
        <AlertDialog.Action
          disabled={!resumeOnLaunch.canResume}
          onclick={() => resumeOnLaunch.resume()}
        >
          Resume
        </AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .list {
    display: flex;
    flex-direction: column;
    /* Shrinks below its content inside the capped dialog, so it scrolls. */
    min-height: 0;
    border: 1px solid var(--border-muted);
    border-radius: 0.5rem;
    overflow: hidden;
  }

  .list-header {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.375rem 0.875rem 0.375rem 0.875rem;
    border-bottom: 1px solid var(--border-muted);
    font-size: 0.8125rem;
    color: var(--text-secondary);
  }

  .select-all {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    cursor: pointer;
    user-select: none;
  }

  .count {
    font-variant-numeric: tabular-nums;
  }

  .rows {
    margin: 0;
    padding: 0.25rem;
    list-style: none;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }
</style>
