<script lang="ts">
  import type { AcpConfigSelector, AcpConfigValueOption } from '../../api/commands';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';

  interface OptionGroup {
    label: string | null;
    options: AcpConfigValueOption[];
  }

  interface Props {
    title: string;
    selector: AcpConfigSelector;
    value: string | null;
    onValueChange: (value: string) => void;
    disabled?: boolean;
  }

  let { title, selector, value, onValueChange, disabled = false }: Props = $props();

  let selectedValue = $derived(value ?? selector.currentValueId);
  let groups = $derived(groupOptions(selector.options));

  function groupOptions(options: AcpConfigValueOption[]): OptionGroup[] {
    const grouped: OptionGroup[] = [];
    for (const option of options) {
      const label = option.groupLabel ?? null;
      let group = grouped[grouped.length - 1];
      if (!group || group.label !== label) {
        group = { label, options: [] };
        grouped.push(group);
      }
      group.options.push(option);
    }
    return grouped;
  }
</script>

<DropdownMenu.Label class="picker-section-label">{title}</DropdownMenu.Label>
{#if selector.options.length > 0}
  <DropdownMenu.RadioGroup
    value={selectedValue ?? undefined}
    onValueChange={(next) => {
      if (!disabled) onValueChange(next);
    }}
  >
    {#each groups as group, groupIndex (`${group.label ?? 'ungrouped'}-${groupIndex}`)}
      {#if group.label}
        <div class="picker-group-label">{group.label}</div>
      {/if}
      {#each group.options as option (option.valueId)}
        <DropdownMenu.RadioItem value={option.valueId} {disabled} closeOnSelect={false}>
          <span class="picker-option-label">{option.label}</span>
        </DropdownMenu.RadioItem>
      {/each}
    {/each}
  </DropdownMenu.RadioGroup>
{:else}
  <DropdownMenu.Item disabled>
    <span class="picker-default-label">Default</span>
  </DropdownMenu.Item>
{/if}

<style>
  :global(.picker-section-label) {
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .picker-group-label {
    padding: 4px 8px 2px;
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  .picker-option-label,
  .picker-default-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
