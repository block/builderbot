<!--
  BranchCardActionsBar.svelte - Header actions bar for a branch card

  Displays running action buttons, primary run action button/pill,
  and the "more" dropdown menu with Actions and Open In submenus.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { slide, fade } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import Play from '@lucide/svelte/icons/play';
  import Hammer from '@lucide/svelte/icons/hammer';
  import FlaskConical from '@lucide/svelte/icons/flask-conical';
  import Check from '@lucide/svelte/icons/check';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import Wrench from '@lucide/svelte/icons/wrench';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import StopCircle from '@lucide/svelte/icons/stop-circle';
  import Copy from '@lucide/svelte/icons/copy';
  import Zap from '@lucide/svelte/icons/zap';
  import Wand2 from '@lucide/svelte/icons/wand-2';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import MoreVertical from '@lucide/svelte/icons/more-vertical';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import ActionOutputModal from '../actions/ActionOutputModal.svelte';
  import type { UnlistenFn } from '../../transport';
  import type { Branch, ProjectRepo } from '../../types';
  import * as commands from '../../api/commands';
  import type { ProjectAction } from '../../api/commands';
  import {
    runBranchAction,
    getRunningBranchActions,
    clearActionExecution,
    stopBranchAction,
    getRunPhase,
    listenToRepoActionsDetection,
    type ActionStatusEvent,
    type ActionType,
    type RunPhase,
  } from '../actions/actions';
  import { getAvailableOpeners, openInApp, copyPathToClipboard, type OpenerApp } from './branch';
  import {
    getPrimaryActionExecution,
    getPrimaryRunAction,
    getRemainingRunActions,
    getSecondaryRunningActions,
    groupActionsByType,
  } from './branchCardHelpers';
  import { toast } from 'svelte-sonner';
  import { bloxEnv } from '../../stores/bloxEnv.svelte';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { onBranchActionStatus, onBranchRunPhaseChanged } from '../../services/branchEventService';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { Button } from '$lib/components/ui/button';

  type MenuIconComponent = typeof MoreVertical;
  type ActionMenuItem = {
    type: 'action';
    label: string;
    icon?: MenuIconComponent;
    iconSrc?: string;
    disabled?: boolean;
    danger?: boolean;
    onSelect: () => void | Promise<void>;
  };
  type SeparatorMenuItem = { type: 'separator' };
  type SubmenuMenuItem = {
    type: 'submenu';
    label: string;
    icon?: MenuIconComponent;
    disabled?: boolean;
    children: MenuItem[];
  };
  type MenuItem = ActionMenuItem | SeparatorMenuItem | SubmenuMenuItem;

  interface Props {
    branch: Branch;
    repoLabel?: ProjectRepo | null;
    isLocal: boolean;
    isRemote: boolean;
    isSettingUp: boolean;
    remoteWorkspaceStatus: string | null;
    onDelete?: () => void;
    onRename?: (branchName: string) => void;
    onNoteCreated?: () => void;
    onRebaseBranch?: () => void;
    onSquashCommits?: () => void;
    newCommitDisabled?: boolean;
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
    newCommitDisabled = false,
    commitCount = 0,
  }: Props = $props();

  const actionMenuTypes = ['run', 'build', 'format', 'check', 'test', 'cleanUp', 'prerun'] as const;

  // Custom transition combining slide and fade effects
  function slideAndFade(
    node: Element,
    { duration = 300, axis = 'x' }: { duration?: number; axis?: 'x' | 'y' } = {}
  ) {
    const style = getComputedStyle(node);
    const opacity = +style.opacity;
    const primaryDimension = axis === 'y' ? 'height' : 'width';
    const primaryDimensionValue = parseFloat(style[primaryDimension]);
    const paddingStart = axis === 'y' ? 'paddingTop' : 'paddingLeft';
    const paddingEnd = axis === 'y' ? 'paddingBottom' : 'paddingRight';
    const marginStart = axis === 'y' ? 'marginTop' : 'marginLeft';
    const marginEnd = axis === 'y' ? 'marginBottom' : 'marginRight';

    return {
      duration,
      easing: cubicOut,
      css: (t: number) => {
        return [
          `overflow: hidden`,
          `opacity: ${t * opacity}`,
          `${primaryDimension}: ${t * primaryDimensionValue}px`,
          `padding-${paddingStart.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[paddingStart])}px`,
          `padding-${paddingEnd.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[paddingEnd])}px`,
          `margin-${marginStart.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[marginStart])}px`,
          `margin-${marginEnd.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[marginEnd])}px`,
        ].join(';');
      },
    };
  }

  function notifyError(title: string, e: unknown): void {
    toast.error(title, {
      description: e instanceof Error ? e.message : String(e),
      duration: Infinity,
    });
  }

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

  // =========================================================================
  // Alt-key tracking (for quick stop action)
  // =========================================================================
  let altHeld = $state(false);

  function handleAltDown(e: KeyboardEvent) {
    if (e.key === 'Alt') altHeld = true;
  }
  function handleAltUp(e: KeyboardEvent) {
    if (e.key === 'Alt') altHeld = false;
  }

  // Actions state
  let actions = $state<ProjectAction[]>([]);

  type RunningAction = {
    executionId: string;
    actionId: string;
    actionName: string;
    actionType: ActionType;
    status: 'running' | 'completed' | 'failed' | 'stopped';
    exitCode?: number | null;
    startedAt?: number;
    completedAt?: number | null;
    fading?: boolean;
  };
  let runningActions = $state<RunningAction[]>([]);
  let actionOutputModal = $state<{
    executionId: string;
    actionId: string;
    actionName: string;
    isStopping: boolean;
  } | null>(null);
  let stoppingExecutions = $state<Set<string>>(new Set());

  // Run phase tracking for run actions (building, running, endpoint detection)
  let runPhases = $state(new Map<string, RunPhase>());

  // Tracks which endpoint copy buttons are showing the "copied" tick
  let endpointCopied = $state<Record<string, boolean>>({});
  let endpointCopiedTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  // More menu state
  let openerApps = $state<OpenerApp[]>([]);

  let unlistenRepoActionsDetection: UnlistenFn | null = null;

  function handleActionsChanged(event: CustomEvent) {
    if (!event.detail?.projectId || event.detail?.projectId === branch.projectId) {
      loadActions();
    }
  }

  $effect(() => {
    const branchId = branch.id;

    const unlistenActionStatus = onBranchActionStatus(branchId, (payload: ActionStatusEvent) => {
      const existingIndex = runningActions.findIndex((a) => a.executionId === payload.executionId);

      if (payload.status === 'running') {
        if (existingIndex === -1) {
          runningActions.push({
            executionId: payload.executionId,
            actionId: payload.actionId,
            actionName: payload.actionName,
            actionType: payload.actionType,
            status: 'running',
            startedAt: payload.startedAt ?? Date.now(),
          });
        }
      } else {
        // Action completed/failed/stopped - update status
        if (existingIndex !== -1) {
          runningActions[existingIndex].status = payload.status as any;
          runningActions[existingIndex].exitCode = payload.exitCode;
          runningActions[existingIndex].completedAt = payload.completedAt;

          // Clean up stopping state and run phase when action reaches terminal state
          if (
            payload.status === 'stopped' ||
            payload.status === 'completed' ||
            payload.status === 'failed'
          ) {
            const updated = new Set(stoppingExecutions);
            updated.delete(payload.executionId);
            stoppingExecutions = updated;

            runPhases.delete(payload.executionId);
            runPhases = new Map(runPhases);
          }

          // Auto-remove terminal states after a delay
          const action = runningActions[existingIndex];
          const isPrimaryAction = primaryRunAction && action.actionId === primaryRunAction.id;

          // Determine delay based on status: completed shows briefly, stopped/failed show longer
          let displayTime: number;
          if (payload.status === 'completed') {
            displayTime = isPrimaryAction ? 1000 : 2000;
          } else {
            // stopped/failed: show status briefly then clean up so rerun works cleanly
            displayTime = isPrimaryAction ? 2000 : 3000;
          }

          setTimeout(() => {
            const foundAction = runningActions.find((a) => a.executionId === payload.executionId);
            if (foundAction && !isPrimaryAction) {
              // Secondary actions fade out
              foundAction.fading = true;
            }
            // Remove after animation completes (or immediately for primary)
            setTimeout(
              () => {
                runningActions = runningActions.filter(
                  (a) => a.executionId !== payload.executionId
                );
              },
              isPrimaryAction ? 0 : 300
            ); // Match CSS transition duration for secondary
          }, displayTime);
        }
      }
    });

    const unlistenRunPhaseChanged = onBranchRunPhaseChanged(branchId, (event) => {
      runPhases.set(event.executionId, event.phase);
      runPhases = new Map(runPhases);
    });

    return () => {
      unlistenActionStatus();
      unlistenRunPhaseChanged();
    };
  });

  onMount(() => {
    loadActions();
    loadRunningActions();
    getAvailableOpeners().then((apps) => (openerApps = apps));
    window.addEventListener('project-actions-changed', handleActionsChanged as EventListener);

    unlistenRepoActionsDetection = listenToRepoActionsDetection((event) => {
      if (!event.detecting) {
        loadActions();
      }
    });

    window.addEventListener('keydown', handleAltDown);
    window.addEventListener('keyup', handleAltUp);
  });

  onDestroy(() => {
    unlistenRepoActionsDetection?.();
    window.removeEventListener('project-actions-changed', handleActionsChanged as EventListener);
    window.removeEventListener('keydown', handleAltDown);
    window.removeEventListener('keyup', handleAltUp);
    for (const timer of Object.values(endpointCopiedTimers)) clearTimeout(timer);
  });

  async function loadActions() {
    try {
      actions = await commands.listProjectActions(branch.projectId, branch.projectRepoId);
    } catch (e) {
      console.error('Failed to load actions:', e);
      actions = [];
    }
  }

  async function loadRunningActions() {
    try {
      const running = await getRunningBranchActions(branch.id);

      for (const info of running) {
        const existingIndex = runningActions.findIndex((a) => a.executionId === info.executionId);
        if (existingIndex === -1) {
          runningActions.push({
            executionId: info.executionId,
            actionId: info.actionId,
            actionName: info.actionName,
            actionType: info.actionType,
            status: 'running',
            startedAt: info.startedAt,
          });
        }

        try {
          const phase = await getRunPhase(info.executionId);
          if (phase) {
            runPhases.set(info.executionId, phase);
          } else if (info.actionType === 'run') {
            runPhases.set(info.executionId, { type: 'running', endpoint: null });
          }
        } catch {
          // Phase not available for this execution
        }
      }
      runPhases = new Map(runPhases);
    } catch (e) {
      console.error('Failed to load running actions:', e);
    }
  }

  // Group actions by type
  let groupedActions = $derived.by(() => {
    return groupActionsByType(actions);
  });

  let primaryRunAction = $derived.by(() => {
    return getPrimaryRunAction(groupedActions);
  });

  let remainingRunActions = $derived.by(() => {
    return getRemainingRunActions(groupedActions);
  });

  let hasActionsForSubmenu = $derived.by(() => {
    return actionMenuTypes.some((type) => {
      const typeActions = type === 'run' ? remainingRunActions : groupedActions[type];
      return typeActions && typeActions.length > 0;
    });
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

  let primaryActionExecution = $derived.by(() => {
    return getPrimaryActionExecution(runningActions, primaryRunAction?.id ?? null);
  });

  let secondaryRunningActions = $derived.by(() => {
    return getSecondaryRunningActions(runningActions, primaryRunAction?.id ?? null);
  });

  async function handleRunAction(action: ProjectAction) {
    const staleExecutions = runningActions.filter(
      (a) => a.actionId === action.id && a.status !== 'running'
    );
    for (const stale of staleExecutions) {
      clearActionExecution(stale.executionId).catch(() => {});
    }
    runningActions = runningActions.filter(
      (a) => !(a.actionId === action.id && a.status !== 'running')
    );

    const existingExecution = runningActions.find(
      (a) => a.actionId === action.id && a.status === 'running'
    );

    if (existingExecution) {
      actionOutputModal = {
        executionId: existingExecution.executionId,
        actionId: action.id,
        actionName: action.name,
        isStopping: stoppingExecutions.has(existingExecution.executionId),
      };
      return;
    }

    try {
      const provider = getPreferredAgent(agentState.providers) ?? undefined;
      await runBranchAction(branch.id, action.id, provider);
    } catch (e) {
      console.error('Failed to run action:', e);
      notifyError(`Failed to run action "${action.name}"`, e);
    }
  }

  async function handleStopAction(executionId: string, actionName: string) {
    if (stoppingExecutions.has(executionId)) {
      return;
    }

    stoppingExecutions = new Set(stoppingExecutions).add(executionId);

    try {
      await stopBranchAction(executionId);
    } catch (e) {
      const updated = new Set(stoppingExecutions);
      updated.delete(executionId);
      stoppingExecutions = updated;
      console.error(`Failed to stop action ${actionName}:`, e);
      notifyError(`Failed to stop action "${actionName}"`, e);
    }
  }

  function handleShowActionOutput(execution: RunningAction) {
    actionOutputModal = {
      executionId: execution.executionId,
      actionId: execution.actionId,
      actionName: execution.actionName,
      isStopping: stoppingExecutions.has(execution.executionId),
    };
  }

  function handleDeleteFromMenu() {
    onDelete?.();
  }

  function handleRenameFromMenu() {
    const next = window.prompt('Rename branch', branch.branchName);
    if (!next) return;
    const trimmed = next.trim();
    if (!trimmed || trimmed === branch.branchName) return;
    onRename?.(trimmed);
  }

  function getActionIcon(actionType: string) {
    switch (actionType) {
      case 'prerun':
        return Zap;
      case 'run':
        return Play;
      case 'build':
        return Hammer;
      case 'format':
        return Wand2;
      case 'check':
        return CheckCircle;
      case 'test':
        return FlaskConical;
      case 'cleanUp':
        return Wrench;
      default:
        return Wrench;
    }
  }

  function buildActionMenuItems(): MenuItem[] {
    const toActionItem = (type: ActionType, action: ProjectAction): MenuItem => ({
      type: 'action',
      label: action.name,
      icon: getActionIcon(type),
      onSelect: () => handleRunAction(action),
    });

    const formatItems = groupedActions.format.map((a) => toActionItem('format', a));
    const checkItems = groupedActions.check.map((a) => toActionItem('check', a));
    const combineFormatCheck = formatItems.length + checkItems.length > 2;

    const groups: MenuItem[][] = [];
    for (const type of actionMenuTypes) {
      if (combineFormatCheck && type === 'check') continue;
      if (combineFormatCheck && type === 'format') {
        const children: MenuItem[] = [
          ...formatItems,
          ...(formatItems.length && checkItems.length ? [{ type: 'separator' as const }] : []),
          ...checkItems,
        ];
        groups.push([
          {
            type: 'submenu',
            label: 'Format & Check',
            icon: Wand2,
            children,
          },
        ]);
        continue;
      }

      const typeActions = type === 'run' ? remainingRunActions : groupedActions[type];
      if (!typeActions || typeActions.length === 0) continue;
      groups.push(typeActions.map((action) => toActionItem(type, action)));
    }

    const items: MenuItem[] = [];
    for (const group of groups) {
      if (items.length > 0) items.push({ type: 'separator' });
      items.push(...group);
    }
    return items;
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

  let actionMenuItems = $derived.by(() => (hasActionsForSubmenu ? buildActionMenuItems() : []));
  let openInMenuItems = $derived.by(() =>
    isLocal && branch.worktreePath && openerApps.length > 0 ? buildOpenInMenuItems() : []
  );
</script>

<!-- Running actions (excluding primary action) -->
{#if isLocal || (isRemote && remoteWorkspaceStatus === 'running')}
  {#each secondaryRunningActions as execution (execution.executionId)}
    {@const isRunning = execution.status === 'running'}
    {@const isStopping = stoppingExecutions.has(execution.executionId)}
    {@const showStopIcon = altHeld && isRunning && !isStopping}
    {@const phase = runPhases.get(execution.executionId)}
    <div
      class="running-action-container"
      class:fading={execution.fading}
      transition:slideAndFade={{ duration: 300, axis: 'x' }}
    >
      <Button
        variant="ghost"
        class={[
          'h-auto whitespace-nowrap rounded-full border bg-[var(--bg-elevated)] border-[var(--border-muted)] px-3 py-1.5 gap-1.5 text-xs text-foreground hover:bg-[var(--bg-hover)] hover:border-[var(--border-focus)] [&_svg]:!size-3',
          execution.status === 'completed' &&
            'border-[var(--status-added)] text-[var(--status-added)]',
          execution.status === 'failed' && 'border-destructive text-destructive',
          isStopping &&
            'opacity-60 hover:bg-[var(--bg-elevated)] hover:border-[var(--border-muted)]',
          showStopIcon && 'border-destructive text-destructive',
        ]}
        title={isStopping
          ? 'Stopping…'
          : showStopIcon
            ? `Stop ${execution.actionName}`
            : isRunning
              ? `View output for ${execution.actionName}`
              : execution.status === 'completed'
                ? `${execution.actionName} completed`
                : execution.status === 'failed'
                  ? `${execution.actionName} failed`
                  : execution.actionName}
        onclick={() => {
          if (isRunning && altHeld && !isStopping) {
            handleStopAction(execution.executionId, execution.actionName);
          } else {
            handleShowActionOutput(execution);
          }
        }}
      >
        {#if isStopping}
          <Spinner size={12} class="danger" />
        {:else if showStopIcon}
          <StopCircle size={12} />
        {:else if isRunning && phase && phase.type !== 'building' && execution.actionType === 'run'}
          <SineWave size={12} />
        {:else if isRunning}
          <Spinner size={12} />
        {:else if execution.status === 'completed'}
          <CheckCircle size={12} />
        {:else if execution.status === 'failed'}
          <AlertCircle size={12} />
        {:else}
          <StopCircle size={12} />
        {/if}
        {execution.actionName}
      </Button>
    </div>
  {/each}
  <!-- Primary run action button -->
  {#if !isSettingUp && primaryRunAction}
    {@const execution = primaryActionExecution}
    {@const isRunning = execution?.status === 'running'}
    {@const isStopping = execution && stoppingExecutions.has(execution.executionId)}
    {@const showStopIcon = altHeld && isRunning && !isStopping}
    {@const phase = execution ? runPhases.get(execution.executionId) : undefined}
    {@const hasEndpoint = phase?.type === 'running' && !!phase.endpoint && canResolveEndpoint}
    {@const copyUrl =
      hasEndpoint && phase?.type === 'running' && phase.endpoint
        ? getEndpointCopyUrl(phase.endpoint)
        : ''}
    <div
      class="primary-action-container"
      in:slide={{ duration: 300, axis: 'x' }}
      out:slide={{ duration: 300, axis: 'x' }}
    >
      {#if isRunning && hasEndpoint && phase?.type === 'running' && phase.endpoint}
        <!-- Pill-shaped button when running with endpoint -->
        <div class="primary-action-pill">
          <Button
            variant="ghost"
            class={[
              'size-7 rounded-full border-0 bg-transparent text-muted-foreground hover:bg-transparent hover:text-foreground [&_svg]:!size-3.5',
              isStopping && 'opacity-60',
              showStopIcon && 'text-destructive',
            ]}
            title={isStopping
              ? 'Stopping…'
              : showStopIcon
                ? `Stop ${primaryRunAction.name}`
                : `View output for ${primaryRunAction.name}`}
            aria-label={isStopping
              ? 'Stopping'
              : showStopIcon
                ? `Stop ${primaryRunAction.name}`
                : `View output for ${primaryRunAction.name}`}
            onclick={() => {
              if (altHeld && !isStopping && execution) {
                handleStopAction(execution.executionId, primaryRunAction.name);
              } else if (execution) {
                handleShowActionOutput(execution);
              }
            }}
          >
            {#if isStopping}
              <Spinner size={14} class="danger" />
            {:else if showStopIcon}
              <StopCircle size={14} />
            {:else}
              <SineWave size={14} />
            {/if}
          </Button>
          <Button
            variant="ghost"
            class="relative size-7 rounded-none border-0 border-l border-l-[var(--border-muted)] bg-transparent text-muted-foreground hover:bg-[var(--bg-elevated)] hover:text-foreground [&_svg]:!size-3"
            title={`Copy endpoint: ${copyUrl}`}
            aria-label="Copy endpoint"
            onclick={(e) => {
              e.stopPropagation();
              if (phase?.type === 'running' && phase.endpoint && execution && copyUrl) {
                navigator.clipboard.writeText(copyUrl).catch(() => {});
                const id = execution.executionId;
                if (endpointCopiedTimers[id]) clearTimeout(endpointCopiedTimers[id]);
                endpointCopied[id] = true;
                endpointCopiedTimers[id] = setTimeout(() => {
                  delete endpointCopied[id];
                  delete endpointCopiedTimers[id];
                }, 1500);
              }
            }}
          >
            {#if execution && endpointCopied[execution.executionId]}
              <span
                class="copy-icon-wrapper"
                in:fade={{ duration: 150 }}
                out:fade={{ duration: 150 }}
              >
                <Check size={12} />
              </span>
            {:else}
              <span
                class="copy-icon-wrapper"
                in:fade={{ duration: 150 }}
                out:fade={{ duration: 150 }}
              >
                <Copy size={12} />
              </span>
            {/if}
          </Button>
        </div>
      {:else}
        <!-- Standard circular button -->
        <Button
          variant="ghost"
          class={[
            'size-7 rounded-full border-0 bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] [&_svg]:!size-3.5',
            isRunning && 'bg-[var(--bg-hover)] text-muted-foreground hover:bg-[var(--bg-elevated)]',
            execution?.status === 'completed' &&
              'bg-[var(--bg-hover)] text-[var(--status-added)] hover:bg-[var(--bg-hover)]',
            execution?.status === 'failed' &&
              'bg-[var(--bg-hover)] text-destructive hover:bg-[var(--bg-hover)]',
            isStopping && 'opacity-60 hover:bg-[var(--bg-elevated)]',
            showStopIcon && 'text-destructive',
          ]}
          title={isStopping
            ? 'Stopping…'
            : showStopIcon
              ? `Stop ${primaryRunAction.name}`
              : isRunning
                ? `View output for ${primaryRunAction.name}`
                : execution?.status === 'completed'
                  ? `${primaryRunAction.name} completed`
                  : execution?.status === 'failed'
                    ? `${primaryRunAction.name} failed`
                    : primaryRunAction.name}
          aria-label={isStopping
            ? 'Stopping'
            : showStopIcon
              ? `Stop ${primaryRunAction.name}`
              : isRunning
                ? `View output for ${primaryRunAction.name}`
                : execution?.status === 'completed'
                  ? `${primaryRunAction.name} completed`
                  : execution?.status === 'failed'
                    ? `${primaryRunAction.name} failed`
                    : primaryRunAction.name}
          onclick={() => {
            if (isRunning && altHeld && !isStopping && execution) {
              handleStopAction(execution.executionId, primaryRunAction.name);
            } else if (isRunning && execution) {
              handleShowActionOutput(execution);
            } else if (isStopping && execution) {
              handleShowActionOutput(execution);
            } else {
              handleRunAction(primaryRunAction);
            }
          }}
        >
          {#if isStopping}
            <Spinner size={14} class="danger" />
          {:else if showStopIcon}
            <StopCircle size={14} />
          {:else if isRunning && phase?.type === 'building'}
            <Spinner size={14} />
          {:else if isRunning}
            <SineWave size={14} />
          {:else if execution?.status === 'completed'}
            <CheckCircle size={14} />
          {:else if execution?.status === 'failed'}
            <AlertCircle size={14} />
          {:else}
            <Play size={14} />
          {/if}
        </Button>
      {/if}
    </div>
  {/if}
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
      {#if hasActionsForSubmenu}
        <DropdownMenu.Sub>
          <DropdownMenu.SubTrigger>
            <Play size={14} /> Actions
          </DropdownMenu.SubTrigger>
          <DropdownMenu.SubContent class="min-w-[160px]">
            {#each actionMenuItems as item, i (i)}
              {#if item.type === 'separator'}
                <DropdownMenu.Separator />
              {:else if item.type === 'submenu'}
                <DropdownMenu.Sub>
                  <DropdownMenu.SubTrigger>
                    {#if item.icon}
                      {@const Icon = item.icon}
                      <Icon size={14} />
                    {/if}
                    {item.label}
                  </DropdownMenu.SubTrigger>
                  <DropdownMenu.SubContent class="min-w-[160px]">
                    {@render renderSubItems(item.children)}
                  </DropdownMenu.SubContent>
                </DropdownMenu.Sub>
              {:else}
                <DropdownMenu.Item disabled={item.disabled} onSelect={item.onSelect}>
                  {#if item.icon}
                    {@const Icon = item.icon}
                    <Icon size={14} />
                  {/if}
                  {item.label}
                </DropdownMenu.Item>
              {/if}
            {/each}
          </DropdownMenu.SubContent>
        </DropdownMenu.Sub>
      {/if}
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
      <DropdownMenu.Item disabled={newCommitDisabled} onSelect={() => onRebaseBranch?.()}>
        <GitBranch size={14} /> Rebase Branch
      </DropdownMenu.Item>
      {#if commitCount >= 2}
        <DropdownMenu.Item disabled={newCommitDisabled} onSelect={() => onSquashCommits?.()}>
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

<ActionOutputModal
  open={actionOutputModal !== null}
  executionId={actionOutputModal?.executionId ?? ''}
  branchId={branch.id}
  actionName={actionOutputModal?.actionName ?? ''}
  isStopping={actionOutputModal?.isStopping}
  onClose={() => (actionOutputModal = null)}
  onRunAgain={async () => {
    const action = actions.find((a) => a.id === actionOutputModal?.actionId);
    if (!action) return;

    // Clean up stale executions of this action
    const staleExecutions = runningActions.filter(
      (a) => a.actionId === action.id && a.status !== 'running'
    );
    for (const stale of staleExecutions) {
      clearActionExecution(stale.executionId).catch(() => {});
    }
    runningActions = runningActions.filter(
      (a) => !(a.actionId === action.id && a.status !== 'running')
    );

    // If already running, just switch the modal to that execution
    const existingExecution = runningActions.find(
      (a) => a.actionId === action.id && a.status === 'running'
    );
    if (existingExecution) {
      actionOutputModal = {
        executionId: existingExecution.executionId,
        actionId: action.id,
        actionName: action.name,
        isStopping: stoppingExecutions.has(existingExecution.executionId),
      };
      return;
    }

    try {
      const provider = getPreferredAgent(agentState.providers) ?? undefined;
      const newExecutionId = await runBranchAction(branch.id, action.id, provider);
      // Keep the modal open and switch to the new execution
      actionOutputModal = {
        executionId: newExecutionId,
        actionId: action.id,
        actionName: action.name,
        isStopping: false,
      };
    } catch (e) {
      console.error('Failed to run action:', e);
      notifyError(`Failed to run action "${action.name}"`, e);
    }
  }}
  {onNoteCreated}
/>

<style>
  /* Primary action button — circular icon-only */
  .primary-action-container {
    display: flex;
    align-items: center;
    overflow: hidden;
  }

  /* Primary action pill (endpoint running state) */
  .primary-action-pill {
    display: flex;
    align-items: center;
    height: 28px;
    background: var(--bg-hover);
    border-radius: 999px;
    overflow: hidden;
  }

  .copy-icon-wrapper {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* Running actions */
  .running-action-container {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow: hidden;
  }

  .running-action-container.fading {
    opacity: 0;
    transform: scale(0.95);
    transition:
      opacity 0.3s ease,
      transform 0.3s ease;
  }
</style>
