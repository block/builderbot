<!--
  ActionsSubmenu.svelte — the "Actions" submenu for a card's more menu.

  Renders a DropdownMenu.Sub listing the runner's actions (built by
  buildActionMenuItems), so it must sit inside a DropdownMenu.Content.
  Renders nothing when the scope has no actions beyond its pinned ones.
-->
<script lang="ts">
  import Play from '@lucide/svelte/icons/play';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { ActionRunner } from './actionRunner.svelte';
  import { buildActionMenuItems, type MenuItem } from './actionMenu';

  interface Props {
    runner: ActionRunner;
  }

  let { runner }: Props = $props();

  let items = $derived(
    buildActionMenuItems(runner.groupedActions, runner.pinnedActionIds, (action) =>
      runner.runAction(action)
    )
  );
</script>

{#snippet renderSubItems(subItems: MenuItem[])}
  {#each subItems as item, i (i)}
    {#if item.type === 'separator'}
      <DropdownMenu.Separator />
    {:else if item.type === 'action'}
      <DropdownMenu.Item disabled={item.disabled} onSelect={item.onSelect}>
        {#if item.icon}
          {@const Icon = item.icon}
          <Icon size={14} />
        {/if}
        {item.label}
      </DropdownMenu.Item>
    {/if}
  {/each}
{/snippet}

{#if items.length > 0}
  <DropdownMenu.Sub>
    <DropdownMenu.SubTrigger>
      <Play size={14} /> Actions
    </DropdownMenu.SubTrigger>
    <DropdownMenu.SubContent class="min-w-[160px]">
      {#each items as item, i (i)}
        {#if item.type === 'separator'}
          <DropdownMenu.Separator />
        {:else if item.type === 'submenu'}
          <DropdownMenu.Sub>
            <DropdownMenu.SubTrigger>
              {#if item.icon}
                {@const Icon = item.icon}
                <Icon size={14} />
              {/if}
              {item.label}
            </DropdownMenu.SubTrigger>
            <DropdownMenu.SubContent class="min-w-[160px]">
              {@render renderSubItems(item.children)}
            </DropdownMenu.SubContent>
          </DropdownMenu.Sub>
        {:else}
          <DropdownMenu.Item disabled={item.disabled} onSelect={item.onSelect}>
            {#if item.icon}
              {@const Icon = item.icon}
              <Icon size={14} />
            {/if}
            {item.label}
          </DropdownMenu.Item>
        {/if}
      {/each}
    </DropdownMenu.SubContent>
  </DropdownMenu.Sub>
{/if}
