<!--
  App.svelte — Root component for Staged.

  Simple view router: home <-> diff.
  Checks CLI launch args to potentially skip straight to diff view.
-->
<script lang="ts">
  import HomePage from './lib/HomePage.svelte';
  import DiffPage from './lib/DiffPage.svelte';
  import * as commands from './lib/commands';
  import type { DiffSpec } from './lib/commands';

  type View = { kind: 'home' } | { kind: 'diff'; spec: DiffSpec; label: string };

  let view = $state<View>({ kind: 'home' });
  let initialized = $state(false);

  async function init() {
    try {
      const args = await commands.getLaunchArgs();

      if (args.mode) {
        let spec: DiffSpec;
        let label: string;

        switch (args.mode) {
          case 'all':
            spec = commands.specUncommitted();
            label = 'All Changes';
            break;
          case 'branch':
            spec = commands.specBranch();
            label = 'Full Branch';
            break;
          case 'commit':
            if (args.commit) {
              spec = commands.specCommit(args.commit);
              label = `Commit ${args.commit.slice(0, 7)}`;
            } else {
              spec = commands.specCommit('HEAD');
              label = 'Last Commit';
            }
            break;
          default:
            spec = commands.specUncommitted();
            label = 'All Changes';
        }

        view = { kind: 'diff', spec, label };
      }
    } catch (e) {
      console.error('Failed to get launch args:', e);
    } finally {
      initialized = true;
    }
  }

  init();

  function openDiff(spec: DiffSpec, label: string) {
    view = { kind: 'diff', spec, label };
  }

  function goHome() {
    view = { kind: 'home' };
  }
</script>

{#if initialized}
  {#if view.kind === 'home'}
    <HomePage onOpenDiff={openDiff} />
  {:else}
    <DiffPage spec={view.spec} label={view.label} onBack={goHome} />
  {/if}
{/if}
