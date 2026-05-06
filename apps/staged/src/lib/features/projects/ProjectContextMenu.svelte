<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { Mail, Trash2 } from 'lucide-svelte';
  import ContextMenu from '../../shared/menu/ContextMenu.svelte';
  import type { MenuItem } from '../../shared/menu/types';

  interface Props {
    x: number;
    y: number;
    onMarkAsUnread: () => void;
    onRemoveProject: () => void;
    onClose: () => void;
  }

  let { x, y, onMarkAsUnread, onRemoveProject, onClose }: Props = $props();

  let contextMenu = $state<ReturnType<typeof ContextMenu> | undefined>();
  let mounted = false;

  let items = $derived<MenuItem[]>([
    {
      type: 'action',
      label: 'Mark as Unread',
      icon: Mail,
      onSelect: onMarkAsUnread,
    },
    {
      type: 'action',
      label: 'Remove Project',
      icon: Trash2,
      danger: true,
      onSelect: onRemoveProject,
    },
  ]);

  async function openMenu() {
    await tick();
    contextMenu?.open({
      x,
      y,
      items,
      ariaLabel: 'Project actions',
    });
  }

  onMount(() => {
    mounted = true;
    void openMenu();
  });

  $effect(() => {
    x;
    y;
    items;
    if (mounted) {
      void openMenu();
    }
  });
</script>

<ContextMenu bind:this={contextMenu} ariaLabel="Project actions" minWidth={172} {onClose} />
