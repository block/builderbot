<!--
  NewTagGroupMenu.svelte — "New group" in Settings › Tags: start a group from
  a template (Status, Priority) or blank.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Plus from '@lucide/svelte/icons/plus';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import TagIcon from '../../tags/TagIcon.svelte';
  import { TAG_GROUP_TEMPLATES, type TagGroupTemplate } from '../../tags/tagTemplates';

  interface Props {
    disabled?: boolean;
    onCreate: (template: TagGroupTemplate) => void;
  }

  let { disabled = false, onCreate }: Props = $props();
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger class="new-tag-group-trigger touch-hit" {disabled}>
    <Plus size={14} />
    New group
    <ChevronDown size={13} class="new-tag-group-chevron" />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end" class="min-w-[220px]">
    <DropdownMenu.Label>Start from</DropdownMenu.Label>
    {#each TAG_GROUP_TEMPLATES as template (template.id)}
      <DropdownMenu.Item onSelect={() => onCreate(template)}>
        <span class="flex min-w-0 flex-1 flex-col">
          <span>{template.label}</span>
          <span class="truncate text-xs text-muted-foreground">{template.description}</span>
        </span>
        {#if template.tags.length > 0}
          <span class="template-preview" aria-hidden="true">
            {#each template.tags.slice(0, 4) as tag, i (i)}
              <TagIcon tag={{ icon: tag.icon ?? null, color: tag.color ?? 'gray' }} size={12} />
            {/each}
          </span>
        {/if}
      </DropdownMenu.Item>
    {/each}
  </DropdownMenu.Content>
</DropdownMenu.Root>

<style>
  :global(.new-tag-group-trigger) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    height: 28px;
    padding: 0 8px 0 10px;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  :global(.new-tag-group-trigger:hover:not(:disabled)),
  :global(.new-tag-group-trigger[data-state='open']) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  :global(.new-tag-group-trigger:disabled) {
    cursor: default;
    opacity: 0.6;
  }

  :global(.new-tag-group-trigger .new-tag-group-chevron) {
    color: var(--text-muted);
  }

  .template-preview {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }
</style>
