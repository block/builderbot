<!--
  BoardEmptyState.svelte — the board before there are any tag groups: what a
  board is, and a button per group template (Status, Priority, Blank) that
  creates one and boards by it.
-->
<script lang="ts">
  import Columns3 from '@lucide/svelte/icons/columns-3';
  import { Button } from '$lib/components/ui/button';
  import { tagsStore } from '../../stores/tags.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { TAG_GROUP_TEMPLATES, freeGroupName, type TagGroupTemplate } from '../tags/tagTemplates';
  import { setBoardGroup } from './boardViewPrefs.svelte';

  let creating = $state(false);

  async function create(template: TagGroupTemplate) {
    if (creating) return;
    creating = true;
    const group = await tagActions.createTagGroup(
      freeGroupName(template, tagsStore.groups),
      template.tags
    );
    creating = false;
    if (group) setBoardGroup(group.id);
  }
</script>

<div class="board-empty">
  <Columns3 size={20} />
  <h3>No tag groups yet</h3>
  <p>
    A board has a column for each tag in a group, like Status or Priority. Start a group from a
    template:
  </p>
  <div class="templates">
    {#each TAG_GROUP_TEMPLATES as template (template.id)}
      <Button
        variant="outline"
        size="sm"
        disabled={creating}
        title={template.description}
        onclick={() => void create(template)}
      >
        {#if template.tags.length > 0}
          <span class="template-preview" aria-hidden="true">
            {#each template.tags.slice(0, 3) as tag, i (i)}
              <TagIcon tag={{ icon: tag.icon ?? null, color: tag.color ?? 'gray' }} size={12} />
            {/each}
          </span>
        {/if}
        {template.label}
      </Button>
    {/each}
  </div>
</div>

<style>
  .board-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    max-width: 420px;
    margin: 32px auto;
    padding: 24px;
    border: 1px dashed var(--border-muted);
    border-radius: 10px;
    color: var(--text-muted);
    text-align: center;
  }

  h3 {
    margin: 0;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-weight: 600;
  }

  p {
    margin: 0;
    font-size: var(--size-sm);
  }

  .templates {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 6px;
    margin-top: 4px;
  }

  .template-preview {
    display: inline-flex;
    align-items: center;
    gap: 1px;
  }
</style>
