<!--
  ChildSessionBadge – "⟳ commit in staged" pill for one in-flight repo session
  spawned by a project session.

  Shaped like a hashtag badge, with a spinner where that badge puts its type
  icon. Tinted with the *repo's* hue rather than the session type's colour: the
  type is already spelled out in the label, so the colour is what tells two
  concurrent children apart. Passive — no click target.
-->
<script lang="ts">
  import Spinner from '../../shared/Spinner.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import { badgeBg, badgeFg } from '../../shared/badgeColors';

  interface Props {
    sessionType: string;
    /** Repo name, or `null` for a child whose branch has no project repo. */
    shortName: string | null;
    /** Repo badge hue, or `null` while the badge is still being generated. */
    hue: number | null;
  }

  let { sessionType, shortName, hue }: Props = $props();

  let tint = $derived(
    hue === null
      ? ''
      : `background: ${badgeBg(hue, darkMode.value)}; color: ${badgeFg(hue, darkMode.value)};`
  );
</script>

<span class="child-session-badge stable-raster" class:untinted={hue === null} style={tint}>
  <Spinner size={12} />
  <span>{shortName ? `${sessionType} in ${shortName}` : sessionType}</span>
</span>

<style>
  /* Matches .hashtag-badge in app.css, as an inline-flex so the spinner and
     label share a baseline the way that badge's icon and text do. */
  .child-session-badge {
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
    gap: 3px;
    padding: 3px 6px;
    border-radius: 4px;
    font-size: 0.85em;
    font-weight: 500;
    line-height: 1.35;
    white-space: nowrap;
  }

  /* No repo badge yet: stay legible rather than guessing a hue. */
  .child-session-badge.untinted {
    background: var(--bg-elevated);
    color: var(--text-muted);
  }
</style>
