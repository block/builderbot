<!--
  BranchCardActionsBar.svelte - Header actions bar for a branch card

  Displays running action buttons, primary run action button/pill,
  and the "more" dropdown menu with Actions and Open In submenus.

  The action-running machinery (state machine, pills, primary button, Actions
  submenu, output modal plumbing) is the shared ActionRunner from the actions
  feature, scoped here to the branch id.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import Copy from '@lucide/svelte/icons/copy';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import MoreVertical from '@lucide/svelte/icons/more-vertical';
  import ActionOutputModal from '../actions/ActionOutputModal.svelte';
  import ActionsSubmenu from '../actions/ActionsSubmenu.svelte';
  import PrimaryRunActionButton from '../actions/PrimaryRunActionButton.svelte';
  import RunningActionPills from '../actions/RunningActionPills.svelte';
  import { ActionRunner } from '../actions/actionRunner.svelte';
  import type { MenuItem } from '../actions/actionMenu';
  import type { UnlistenFn } from '../../transport';
  import type { Branch, ProjectRepo } from '../../types';
  import * as commands from '../../api/commands';
  import { runBranchAction, listenToRepoActionsDetection } from '../actions/actions';
  import { getAvailableOpeners, openInApp, copyPathToClipboard, type OpenerApp } from './branch';
  import { bloxEnv } from '../../stores/bloxEnv.svelte';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState } from '../agents/agent.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import RenameBranchDialog from './RenameBranchDialog.svelte';

  interface Props {
    branch: Branch;
    repoLabel?: ProjectRepo | null;
    isLocal: boolean;
    isRemote: boolean;
    isSettingUp: boolean;
    remoteWorkspaceStatus: string | null;
    onDelete?: () => void;
    onRename?: (branchName: string) => void | Promise<void>;
    onNoteCreated?: () => void;
    onRebaseBranch?: () => void;
    onSquashCommits?: () => void;
    /** Rebase/Squash queue behind running sessions, so this covers only the
     *  cases where they can't run at all (detached HEAD, wrong branch). */
    rebaseSquashDisabled?: boolean;
    commitCount?: number;
  }

  let {
    branch,
    repoLabel = null,
    isLocal,
    isRemote,
    isSettingUp,
    remoteWorkspaceStatus,
    onDelete,
    onRename,
    onNoteCreated,
    onRebaseBranch,
    onSquashCommits,
    rebaseSquashDisabled = false,
    commitCount = 0,
  }: Props = $props();

  // =========================================================================
  // Remote endpoint URL rewriting
  // =========================================================================
  let canResolveEndpoint = $derived(!isRemote || !!branch.workstationId);

  function getEndpointCopyUrl(endpoint: string): string {
    if (!isRemote) return endpoint;
    if (!canResolveEndpoint) return endpoint;
    try {
      const parsed = new URL(endpoint);
      const port = parsed.port || (parsed.protocol === 'https:' ? '443' : '80');
      const path = parsed.pathname + parsed.search + parsed.hash;
      const domain =
        bloxEnv.value === 'staging' ? 'blox.stage.blox.sqprod.co' : 'blox.blox.sqprod.co';
      return `https://workstation-${branch.workstationId}-${port}--${domain}${path}`;
    } catch {
      return endpoint;
    }
  }

  // Actions state — the shared runner, scoped to this branch's id
  const runner = new ActionRunner({
    getScopeId: () => branch.id,
    loadActions: () => commands.listProjectActions(branch.projectId, branch.projectRepoId),
    run: (actionId) =>
      runBranchAction(branch.id, actionId, getPreferredAgent(agentState.providers) ?? undefined),
  });

  let renameDialogOpen = $state(false);

  // More menu state
  let openerApps = $state<OpenerApp[]>([]);

  let unlistenRepoActionsDetection: UnlistenFn | null = null;

  function handleActionsChanged(event: CustomEvent) {
    if (!event.detail?.projectId || event.detail?.projectId === branch.projectId) {
      runner.loadActions();
    }
  }

  $effect(() => runner.subscribe());

  onMount(() => {
    runner.loadActions();
    runner.loadRunningActions();
    getAvailableOpeners().then((apps) => (openerApps = apps));
    window.addEventListener('project-actions-changed', handleActionsChanged as EventListener);

    unlistenRepoActionsDetection = listenToRepoActionsDetection((event) => {
      if (!event.detecting) {
        runner.loadActions();
      }
    });
  });

  onDestroy(() => {
    unlistenRepoActionsDetection?.();
    window.removeEventListener('project-actions-changed', handleActionsChanged as EventListener);
  });

  async function handleOpenInApp(appId: string) {
    if (branch.worktreePath) {
      await openInApp(branch.worktreePath, appId);
    }
  }

  async function handleCopyPath() {
    if (branch.worktreePath) {
      await copyPathToClipboard(branch.worktreePath);
    }
  }

  function handleDeleteFromMenu() {
    onDelete?.();
  }

  function handleRenameFromMenu() {
    renameDialogOpen = true;
  }

  const terminalAppIds = new Set([
    'terminal',
    'warp',
    'iterm',
    'hyper',
    'kitty',
    'alacritty',
    'ghostty',
  ]);
  const fileBrowserAppIds = new Set(['finder']);

  function buildOpenInMenuItems(): MenuItem[] {
    const terminals: MenuItem[] = [];
    const editors: MenuItem[] = [];
    const fileBrowsers: MenuItem[] = [];

    for (const app of openerApps) {
      const item: MenuItem = {
        type: 'action',
        label: app.name,
        iconSrc: app.icon ?? undefined,
        onSelect: () => handleOpenInApp(app.id),
      };
      if (terminalAppIds.has(app.id)) {
        terminals.push(item);
      } else if (fileBrowserAppIds.has(app.id)) {
        fileBrowsers.push(item);
      } else {
        editors.push(item);
      }
    }

    const sortByLabel = (a: MenuItem, b: MenuItem) =>
      (a.type === 'action' ? a.label : '').localeCompare(b.type === 'action' ? b.label : '');

    const items: MenuItem[] = [];
    if (terminals.length > 0) items.push(...terminals.sort(sortByLabel));
    if (editors.length > 0) {
      if (items.length > 0) items.push({ type: 'separator' });
      items.push(...editors.sort(sortByLabel));
    }
    if (fileBrowsers.length > 0) {
      if (items.length > 0) items.push({ type: 'separator' });
      items.push(...fileBrowsers.sort(sortByLabel));
    }

    if (items.length > 0) items.push({ type: 'separator' });
    items.push({
      type: 'action',
      label: 'Copy Path',
      icon: Copy,
      onSelect: handleCopyPath,
    });

    return items;
  }

  let openInMenuItems = $derived.by(() =>
    isLocal && branch.worktreePath && openerApps.length > 0 ? buildOpenInMenuItems() : []
  );
</script>

<!-- Running actions (excluding primary action), then the primary run action button -->
{#if isLocal || (isRemote && remoteWorkspaceStatus === 'running')}
  <RunningActionPills {runner} />
  <PrimaryRunActionButton {runner} show={!isSettingUp} {canResolveEndpoint} {getEndpointCopyUrl} />
{/if}
{#snippet renderSubItems(items: MenuItem[])}
  {#each items as item, i (i)}
    {#if item.type === 'separator'}
      <DropdownMenu.Separator />
    {:else if item.type === 'action'}
      <DropdownMenu.Item disabled={item.disabled} onSelect={item.onSelect}>
        {#if item.icon}
          {@const Icon = item.icon}
          <Icon size={14} />
        {:else if item.iconSrc}
          <img src={item.iconSrc} alt="" width="14" height="14" class="shrink-0 rounded-[3px]" />
        {/if}
        {item.label}
      </DropdownMenu.Item>
    {/if}
  {/each}
{/snippet}

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class="inline-flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-faint)] transition-colors hover:bg-[var(--bg-hover)] hover:text-foreground focus-visible:bg-[var(--bg-hover)] focus-visible:text-foreground focus-visible:outline-none data-[state=open]:bg-[var(--bg-hover)] data-[state=open]:text-foreground disabled:cursor-not-allowed disabled:opacity-45"
    aria-label="More options"
    title="More options"
  >
    <MoreVertical size={16} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[160px]">
    {#if !isSettingUp}
      {#if isRemote && branch.workspaceName}
        <DropdownMenu.Item
          onSelect={() => {
            navigator.clipboard.writeText(branch.workspaceName!);
          }}
        >
          <Copy size={14} /> Copy Workspace Name
        </DropdownMenu.Item>
      {/if}
      <ActionsSubmenu {runner} />
      {#if isLocal && branch.worktreePath && openerApps.length > 0}
        <DropdownMenu.Separator />
        <DropdownMenu.Sub>
          <DropdownMenu.SubTrigger>
            <ExternalLink size={14} /> Open In
          </DropdownMenu.SubTrigger>
          <DropdownMenu.SubContent class="min-w-[160px]">
            {@render renderSubItems(openInMenuItems)}
          </DropdownMenu.SubContent>
        </DropdownMenu.Sub>
      {:else if isLocal && branch.worktreePath}
        <DropdownMenu.Separator />
        <DropdownMenu.Item onSelect={handleCopyPath}>
          <Copy size={14} /> Copy Worktree Path
        </DropdownMenu.Item>
      {/if}
      <DropdownMenu.Separator />
      <DropdownMenu.Item onSelect={handleRenameFromMenu}>
        <GitBranch size={14} /> Rename Branch
      </DropdownMenu.Item>
      <DropdownMenu.Item disabled={rebaseSquashDisabled} onSelect={() => onRebaseBranch?.()}>
        <GitBranch size={14} /> Rebase Branch
      </DropdownMenu.Item>
      {#if commitCount >= 2}
        <DropdownMenu.Item disabled={rebaseSquashDisabled} onSelect={() => onSquashCommits?.()}>
          <GitBranch size={14} /> Squash Commits
        </DropdownMenu.Item>
      {/if}
      <DropdownMenu.Separator />
    {/if}
    <DropdownMenu.Item variant="destructive" onSelect={handleDeleteFromMenu}>
      <Trash2 size={14} /> Delete Repo
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>

<RenameBranchDialog bind:open={renameDialogOpen} branchName={branch.branchName} {onRename} />

<ActionOutputModal
  open={runner.outputModal !== null}
  executionId={runner.outputModal?.executionId ?? ''}
  branchId={branch.id}
  actionName={runner.outputModal?.actionName ?? ''}
  isStopping={runner.outputModal?.isStopping}
  onClose={() => runner.closeOutputModal()}
  onRunAgain={() => runner.runAgain()}
  {onNoteCreated}
/>
