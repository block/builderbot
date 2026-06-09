<!--
  AgentSelector.svelte — Compact dropdown for switching the AI agent.

  Shows the currently selected agent with a chevron. Clicking opens a
  dropdown with all available providers. Selection is saved to preferences.

  Reads from the shared agentState cache (populated once at app startup)
  so there's no discovery delay when opening modals.

  Usage:
    <AgentSelector />
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import AgentIcon from './AgentIcon.svelte';
  import { agentState, REMOTE_AGENTS } from './agent.svelte';
  import { setAiAgent, getPreferredAgent } from '../settings/preferences.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';

  interface Props {
    disabled?: boolean;
    remote?: boolean;
    dropUp?: boolean;
  }

  let { disabled = false, remote = false, dropUp = false }: Props = $props();

  let agents = $derived(remote ? REMOTE_AGENTS : agentState.providers);

  let preferredId = $derived(getPreferredAgent(agents));

  let currentLabel = $derived(agents.find((p) => p.id === preferredId)?.label ?? 'Agent');
</script>

{#if (remote || agentState.loaded) && agents.length > 0}
  {#if agents.length > 1}
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        class="inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-[var(--text-faint)] transition-colors hover:bg-[var(--bg-hover)] hover:text-muted-foreground focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40"
        {disabled}
        title="Select AI agent"
      >
        <AgentIcon id={preferredId ?? ''} size={12} />
        <span class="whitespace-nowrap">{currentLabel}</span>
        <ChevronDown size={12} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Content
        align="start"
        side={dropUp ? 'top' : 'bottom'}
        sideOffset={4}
        class="min-w-[140px]"
      >
        <DropdownMenu.RadioGroup
          value={preferredId ?? undefined}
          onValueChange={(id) => setAiAgent(id)}
        >
          {#each agents as provider (provider.id)}
            <DropdownMenu.RadioItem value={provider.id}>
              <span class="inline-flex items-center gap-1.5">
                <AgentIcon id={provider.id} size={12} />
                {provider.label}
              </span>
            </DropdownMenu.RadioItem>
          {/each}
        </DropdownMenu.RadioGroup>
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  {:else}
    <button
      type="button"
      class="inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-[var(--text-faint)] disabled:cursor-not-allowed disabled:opacity-40"
      {disabled}
      title={currentLabel}
    >
      <AgentIcon id={preferredId ?? ''} size={12} />
      <span class="whitespace-nowrap">{currentLabel}</span>
    </button>
  {/if}
{/if}
