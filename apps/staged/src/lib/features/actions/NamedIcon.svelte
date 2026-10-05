<!--
  NamedIcon.svelte — a Lucide icon by its stored kebab-case name.

  The full icon map is a lazily fetched chunk (see lucideIcons.ts), so a named
  icon can't render on the first frame. Until it arrives — and permanently, if
  the stored name isn't a Lucide icon any more after a rename — this shows the
  caller's fallback, so the slot is never empty.
-->
<script lang="ts">
  import { loadIconComponent, type IconComponent } from './lucideIcons';

  interface Props {
    /** Kebab-case Lucide icon name, or null for the fallback. */
    name: string | null;
    fallback: IconComponent;
    size?: number;
    style?: string;
  }

  let { name, fallback, size = 14, style }: Props = $props();

  let custom = $state<IconComponent | null>(null);

  $effect(() => {
    const iconName = name;
    custom = null;
    if (!iconName) return;

    let cancelled = false;
    loadIconComponent(iconName).then((component) => {
      if (!cancelled) custom = component;
    });
    return () => {
      cancelled = true;
    };
  });

  let Icon = $derived(custom ?? fallback);
</script>

<Icon {size} {style} />
