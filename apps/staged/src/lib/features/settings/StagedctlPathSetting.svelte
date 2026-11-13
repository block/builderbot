<!--
  StagedctlPathSetting.svelte - Settings ▸ Connected apps ▸ Command line

  The action that puts `stagedctl` on the user's PATH. A release install
  links nothing, so without this the CLI has to be linked by hand.

  The backend decides where the link goes and never overwrites what is
  already there: it answers with what it found. A link that points somewhere
  else is shown here with its target, and is replaced only when the user
  presses "Replace link" under it. That press sends back the link and target
  that were on screen, and the backend replaces the link only if it is still
  that one. A file or directory in the way is reported and left alone.

  Desktop only: the panel that renders this is hidden in web mode, and the
  web dispatch refuses the command.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Button } from '$lib/components/ui/button';
  import Spinner from '../../shared/Spinner.svelte';
  import * as commands from '../../api/commands';
  import type { StagedctlLinkOutcome, StagedctlShownLink } from '../../types';

  let outcome = $state<StagedctlLinkOutcome | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let mounted = true;

  onDestroy(() => {
    mounted = false;
  });

  async function link(replace?: StagedctlShownLink) {
    if (busy) return;
    busy = true;
    try {
      const result = await commands.linkStagedctlOnPath(replace);
      if (!mounted) return;
      outcome = result;
      error = null;
    } catch (e) {
      if (!mounted) return;
      outcome = null;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      if (mounted) busy = false;
    }
  }

  /** The directory a link is in: its path without the last segment. */
  function directoryOf(path: string): string {
    const end = path.lastIndexOf('/');
    return end > 0 ? path.slice(0, end) : path;
  }
</script>

<section class="cli-setting" aria-labelledby="stagedctl-path-heading">
  <div class="cli-row">
    <div class="cli-copy">
      <h3 id="stagedctl-path-heading">Command line</h3>
      <p>
        Add <code>stagedctl</code> to your PATH so you can run it from a terminal. Staged links to the
        copy inside this app, so it stays in step with updates.
      </p>
    </div>
    <Button variant="outline" size="sm" disabled={busy} aria-busy={busy} onclick={() => link()}>
      {#if busy}
        <Spinner size={12} />
      {/if}
      Add to PATH
    </Button>
  </div>

  {#if error}
    <p class="cli-notice cli-error" role="alert">{error}</p>
  {/if}

  <!-- Always in the DOM, so a screen reader hears the result when it lands. -->
  <div class="cli-status" role="status" aria-live="polite">
    {#if outcome?.outcome === 'conflict'}
      {#if outcome.existing.kind === 'link'}
        {@const shown = { link: outcome.link, target: outcome.existing.target }}
        <p class="cli-notice" data-testid="stagedctl-conflict">
          <code>{shown.link}</code> already exists and points to <code>{shown.target}</code>.
          Nothing was changed. Replace it to make <code>stagedctl</code> run this copy of Staged.
        </p>
        <Button variant="outline" size="sm" disabled={busy} onclick={() => link(shown)}>
          Replace link
        </Button>
      {:else}
        <p class="cli-notice" data-testid="stagedctl-conflict">
          <code>{outcome.link}</code> already exists and is not a link, so Staged left it alone. Move
          or delete it, then try again.
        </p>
      {/if}
    {:else if outcome}
      {@const directory = directoryOf(outcome.link)}
      <p class="cli-notice" data-testid="stagedctl-linked">
        {#if outcome.outcome === 'linked'}
          Linked <code>{outcome.link}</code> to this copy of Staged.
        {:else}
          <code>{outcome.link}</code> already points to this copy of Staged.
        {/if}
      </p>
      {#if outcome.onPath === false}
        <p class="cli-notice" data-testid="stagedctl-path-hint">
          <code>{directory}</code> is not on your PATH. Add it in your shell profile, then open a new
          terminal.
        </p>
      {:else if outcome.onPath === null}
        <p class="cli-notice" data-testid="stagedctl-path-hint">
          Staged could not read your PATH. If a new terminal cannot find <code>stagedctl</code>, add
          <code>{directory}</code> to your PATH.
        </p>
      {/if}
    {/if}
  </div>
</section>

<style>
  .cli-setting {
    max-width: 640px;
    width: 100%;
    margin-inline: auto;
    /* The panel body is a scrolling flex column: keep this at its own height. */
    flex-shrink: 0;
    padding: 10px 12px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-elevated);
  }

  .cli-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .cli-copy {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .cli-copy h3 {
    margin: 0;
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .cli-copy p {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
  }

  .cli-status {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
  }

  .cli-notice {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
    /* Paths have no spaces to break at. */
    overflow-wrap: anywhere;
  }

  .cli-error {
    color: var(--ui-danger);
  }

  /* The spacing sits on the content, so the live region takes no room
     while it is empty. */
  .cli-status > :first-child,
  .cli-error {
    margin-top: 8px;
  }

  code {
    font-size: 0.9em;
  }

  @media (max-width: 920px) {
    .cli-row {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
