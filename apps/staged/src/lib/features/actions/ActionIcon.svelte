<!--
  ActionIcon.svelte — an action's icon: the Lucide icon it picked, or the
  default for its type.

  The full icon map is a lazily fetched chunk (see lucideIcons.ts), so a custom
  icon can't render on the first frame. Until it arrives — and permanently, if
  the stored name isn't a Lucide icon any more after a rename — this shows the
  action type's default icon, so a card is never icon-less.
-->
<script lang="ts">
  import { getActionTypeIcon, loadIconComponent, type IconComponent } from './lucideIcons';

  interface Props {
    /** Kebab-case Lucide icon name, or null for the action type's default. */
    icon: string | null;
    actionType: string;
    size?: number;
  }

  let { icon, actionType, size = 14 }: Props = $props();

  let custom = $state<IconComponent | null>(null);

  $effect(() => {
    const name = icon;
    custom = null;
    if (!name) return;

    let cancelled = false;
    loadIconComponent(name).then((component) => {
      if (!cancelled) custom = component;
    });
    return () => {
      cancelled = true;
    };
  });

  let Icon = $derived(custom ?? getActionTypeIcon(actionType));
</script>

<Icon {size} />
