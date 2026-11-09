<!--
  AgentEnvVarsSetting.svelte — the environment variables one agent launches
  with, edited under its row in Doctor settings.

  One row per variable: an enabled checkbox, name, value and delete. Rows
  render in stored order and new ones append. Unchecking a row keeps it but
  leaves it out of the agent's environment, so a variable can be switched off
  without retyping it. A row the backend would skip on launch (a name that
  can't be set, a value with a NUL) shows its error inline and stays; enabled
  rows that share a name are all flagged, since only the last one takes
  effect. Values are stored as typed in the local preferences file.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Plus from '@lucide/svelte/icons/plus';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import { Button } from '$lib/components/ui/button';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { Input } from '$lib/components/ui/input';
  import { agentEnvVarRowErrors, type AgentEnvVar } from './agentEnvVars';
  import { getAgentEnvVars, preferences, setAgentEnvVars } from './preferences.svelte';

  let { agentId }: { agentId: string } = $props();

  // Read through `preferences` so a change from another window shows here.
  let entries = $derived(preferences.agentEnvVars[agentId] ?? []);
  // Per row, by position: duplicate detection needs the whole list.
  let rowErrors = $derived(agentEnvVarRowErrors(entries));

  function nameInputId(index: number): string {
    return `agent-env-${agentId}-${index}-name`;
  }

  function updateEntry(index: number, patch: Partial<AgentEnvVar>) {
    setAgentEnvVars(
      agentId,
      getAgentEnvVars(agentId).map((entry, i) => (i === index ? { ...entry, ...patch } : entry))
    );
  }

  function removeEntry(index: number) {
    setAgentEnvVars(
      agentId,
      getAgentEnvVars(agentId).filter((_, i) => i !== index)
    );
  }

  async function addEntry() {
    const current = getAgentEnvVars(agentId);
    setAgentEnvVars(agentId, [...current, { name: '', value: '', enabled: true }]);
    await tick();
    document.getElementById(nameInputId(current.length))?.focus();
  }
</script>

<div class="agent-env-vars">
  <span class="field-label">Environment variables</span>

  {#if entries.length > 0}
    <ul class="env-var-list">
      <!-- Keyed by position: a row's identity is where it sits in the list,
           and the controlled inputs below take their values from state, so a
           removal shifts the rows up without losing anything. -->
      {#each entries as entry, index (index)}
        {@const error = rowErrors[index] ?? null}
        {@const errorId = `${nameInputId(index)}-error`}
        <li class="env-var-row" class:disabled={!entry.enabled}>
          <div class="env-var-fields">
            <Checkbox
              class="data-unchecked:bg-[var(--bg-primary)] dark:data-unchecked:bg-[var(--bg-primary)]"
              checked={entry.enabled}
              onCheckedChange={(checked) => updateEntry(index, { enabled: checked === true })}
              aria-label={entry.enabled ? 'Disable variable' : 'Enable variable'}
              title={entry.enabled ? 'Enabled' : 'Disabled'}
            />
            <Input
              id={nameInputId(index)}
              type="text"
              placeholder="NAME"
              aria-label="Variable name"
              aria-invalid={error !== null}
              aria-describedby={error !== null ? errorId : undefined}
              class="env-var-name h-8 min-w-0 flex-1 font-mono text-xs"
              autocapitalize="off"
              autocorrect="off"
              spellcheck={false}
              value={entry.name}
              oninput={(e) => updateEntry(index, { name: e.currentTarget.value })}
            />
            <span class="env-var-equals" aria-hidden="true">=</span>
            <Input
              type="text"
              placeholder="value"
              aria-label="Variable value"
              aria-describedby={error !== null ? errorId : undefined}
              class="env-var-value h-8 min-w-0 flex-[2] font-mono text-xs"
              autocapitalize="off"
              autocorrect="off"
              spellcheck={false}
              value={entry.value}
              oninput={(e) => updateEntry(index, { value: e.currentTarget.value })}
            />
            <Button
              variant="ghost"
              size="icon-sm"
              class="hover:text-destructive"
              title="Remove variable"
              aria-label="Remove variable"
              onclick={() => removeEntry(index)}
            >
              <Trash2 size={13} />
            </Button>
          </div>
          {#if error !== null}
            <p id={errorId} class="env-var-error" role="alert">{error}</p>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <div class="env-var-actions">
    <Button variant="outline" size="sm" onclick={() => void addEntry()}>
      <Plus size={14} />
      Add variable
    </Button>
    <span class="field-description">
      Set for the agent when it starts. Unchecked rows are kept but not set. Values are stored
      unencrypted in the local preferences file. Local sessions only: remote Blox sessions don’t
      receive them.
    </span>
  </div>
</div>

<style>
  .agent-env-vars {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .field-label {
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-muted);
  }

  .env-var-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .env-var-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .env-var-fields {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* A switched-off row stays editable but reads as inactive. */
  .env-var-row.disabled :global([data-slot='input']) {
    opacity: 0.5;
  }

  .env-var-row.disabled .env-var-equals {
    opacity: 0.5;
  }

  .env-var-equals {
    flex-shrink: 0;
    font-family: monospace;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .env-var-error {
    margin: 0;
    /* Under the name field: past the checkbox and its gap. */
    padding-left: 24px;
    font-size: var(--size-xs);
    color: var(--destructive, var(--color-danger, #f85149));
  }

  .env-var-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .field-description {
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
  }

  @media (pointer: coarse) {
    .env-var-fields {
      flex-wrap: wrap;
    }

    .env-var-fields > :global([data-slot='button']) {
      min-width: 32px;
      min-height: 32px;
    }
  }
</style>
