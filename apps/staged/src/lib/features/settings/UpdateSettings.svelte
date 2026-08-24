<!--
  UpdateSettings.svelte — compact updater section for the General settings panel.

  Pure presentation over the shared updater state from
  `src/lib/services/updater.svelte.ts`: the parent passes the observable
  `UpdaterState` record plus the manual-check callback, so this component has
  no native plugin imports and can be unit-tested with Svelte's server render.

  Shows the installed version, updater availability, a check-for-updates
  action, and every updater status (checking, up-to-date, update available
  with release notes, installing, restart required, recoverable error with
  retry). Updater status is deliberately separate from database-compatibility
  errors — a storage incompatibility is surfaced by the app shell, never here.
-->
<script lang="ts">
  import Info from '@lucide/svelte/icons/info';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import { Button } from '$lib/components/ui/button';
  import Spinner from '../../shared/Spinner.svelte';
  import type { UpdaterState } from '../../services/updater';

  let {
    updater,
    appVersion,
    onCheck,
  }: {
    /** Observable updater status (pass the shared reactive record). */
    updater: UpdaterState;
    /** Currently installed app version, without the leading `v`. */
    appVersion: string;
    /** Explicit manual update check (Settings entry point). */
    onCheck: () => void;
  } = $props();

  /** The updater is doing (or has just finished) work that a new check would disturb. */
  const busy = $derived(
    updater.status === 'checking' ||
      updater.status === 'installing' ||
      updater.status === 'restart-required'
  );

  const statusText = $derived.by(() => {
    switch (updater.status) {
      case 'unavailable':
        return 'Automatic updates are not available in this build.';
      case 'idle':
        return 'Updates are checked automatically in the background.';
      case 'checking':
        return 'Checking for updates…';
      case 'up-to-date':
        return 'Staged is up to date.';
      case 'update-available':
        return updater.availableVersion
          ? `Version ${updater.availableVersion} is available.`
          : 'A new version is available.';
      case 'installing':
        return updater.availableVersion
          ? `Downloading and installing version ${updater.availableVersion}…`
          : 'Downloading and installing the update…';
      case 'restart-required':
        return 'Update installed. Staged will restart to finish.';
      case 'error':
        return updater.error ?? 'The last update check failed.';
    }
  });

  const showReleaseNotes = $derived(
    (updater.status === 'update-available' || updater.status === 'installing') &&
      !!updater.releaseNotes
  );
</script>

<div class="update-settings">
  <span class="field-label">Updates</span>

  <p class="update-version">Staged v{appVersion}</p>

  <p
    class="update-status"
    class:update-status-error={updater.status === 'error'}
    role="status"
    aria-live="polite"
  >
    {#if busy}
      <Spinner size={12} />
    {:else if updater.status === 'error'}
      <TriangleAlert size={12} aria-hidden="true" />
    {/if}
    <span>{statusText}</span>
  </p>

  {#if showReleaseNotes}
    <div class="release-notes">
      <span class="release-notes-label">Release notes</span>
      <pre class="release-notes-body">{updater.releaseNotes}</pre>
    </div>
  {/if}

  {#if updater.status !== 'unavailable'}
    <Button
      variant="outline"
      size="sm"
      class="w-fit"
      disabled={busy}
      aria-busy={busy}
      onclick={onCheck}
    >
      {updater.status === 'error' ? 'Try again' : 'Check for updates'}
    </Button>
  {/if}

  <p class="field-description">
    <Info size={12} />
    {#if updater.status === 'unavailable'}
      Automatic updates only run in official release builds. Development, source, and web builds
      never contact the updater.
    {:else}
      Staged checks for updates in the background and always asks before installing. Updates never
      interrupt your work.
    {/if}
  </p>
</div>

<style>
  .update-settings {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }

  .field-label {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .update-version {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .update-status {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
  }

  .update-status-error {
    color: var(--ui-danger);
  }

  .release-notes {
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 100%;
    max-width: 480px;
  }

  .release-notes-label {
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-primary);
  }

  .release-notes-body {
    margin: 0;
    max-height: 160px;
    overflow-y: auto;
    padding: 8px 10px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: 5px;
    font-family: inherit;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .field-description {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
    display: flex;
    align-items: baseline;
    gap: 4px;
  }
</style>
