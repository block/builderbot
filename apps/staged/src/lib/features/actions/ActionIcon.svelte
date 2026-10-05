<!--
  ActionIcon.svelte — an action's icon: the Lucide icon it picked, or the
  default for its type.

  A picked icon loads lazily (see NamedIcon), so until it arrives — and
  permanently, if the stored name isn't a Lucide icon any more — this shows the
  action type's default icon, so a card is never icon-less.
-->
<script lang="ts">
  import NamedIcon from './NamedIcon.svelte';
  import { getActionTypeIcon } from './lucideIcons';

  interface Props {
    /** Kebab-case Lucide icon name, or null for the action type's default. */
    icon: string | null;
    actionType: string;
    size?: number;
  }

  let { icon, actionType, size = 14 }: Props = $props();
</script>

<NamedIcon name={icon} fallback={getActionTypeIcon(actionType)} {size} />
