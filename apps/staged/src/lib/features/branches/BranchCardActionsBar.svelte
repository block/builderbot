<!--
  BranchCardActionsBar.svelte - Header actions bar for a branch card

  Displays running action buttons, primary run action button/pill,
  and the "more" dropdown menu with Actions and Open In submenus.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { slide, fade } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import {
    GitBranch,
    Play,
    Hammer,
    FlaskConical,
    Check,
    CheckCircle,
    Wrench,
    AlertCircle,
    StopCircle,
    Copy,
    ChevronDown,
    Zap,
    Wand2,
    MoreVertical,
    ExternalLink,
    Trash2,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import ActionOutputModal from '../actions/ActionOutputModal.svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type { Branch, ProjectRepo } from '../../types';
  import * as commands from '../../api/commands';
  import type { ProjectAction } from '../../api/commands';
  import {
    runBranchAction,
    getRunningBranchActions,
    clearActionExecution,
    stopBranchAction,
    getRunPhase,
    listenToRunPhaseChanged,
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
  import { alerts } from '../../shared/alerts.svelte';
  import { bloxEnv } from '../../stores/bloxEnv.svelte';

  interface Props {
    branch: Branch;
    repoLabel?: ProjectRepo | null;
    isLocal: boolean;
    isRemote: boolean;
    remoteWorkspaceStatus: string | null;
    worktreeError?: string;
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
    remoteWorkspaceStatus,
    worktreeError,
    onDelete,
    onRename,
    onNoteCreated,
    onRebaseBranch,
    onSquashCommits,
    newCommitDisabled = false,
    commitCount = 0,
  }: Props = $props();

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
    alerts.show({
      tone: 'error',
      title,
      message: e instanceof Error ? e.message : String(e),
      durationMs: 0,
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
    actionName: string;
    isStopping: boolean;
  } | null>(null);
  let stoppingExecutions = $state<Set<string>>(new Set());

  // Run phase tracking for run actions (building, running, endpoint detection)
  let runPhases = $state(new Map<string, RunPhase>());

  // Tracks which endpoint copy buttons are showing the "copied" tick
  let endpointCopied = $state<Record<string, boolean>>({});
  let endpointCopiedTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  // Dropdown state
  let showMoreMenu = $state(false);
  let showActionsSubmenu = $state(false);
  let actionsSubmenuTimeout = $state<ReturnType<typeof setTimeout> | null>(null);
  let showOpenInSubmenu = $state(false);
  let openInSubmenuTimeout = $state<ReturnType<typeof setTimeout> | null>(null);
  let openerApps = $state<OpenerApp[]>([]);

  let unlistenActionStatus: UnlistenFn | null = null;
  let unlistenRunPhaseChanged: UnlistenFn | null = null;
  let unlistenRepoActionsDetection: UnlistenFn | null = null;

  function handleActionsChanged(event: CustomEvent) {
    if (!event.detail?.projectId || event.detail?.projectId === branch.projectId) {
      loadActions();
    }
  }

  $effect(() => {
    const branchId = branch.id;

    listen<ActionStatusEvent>('action_status', (event) => {
      const payload = event.payload;

      // Only process events for this branch
      if (payload.branchId !== branchId) {
        return;
      }

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
    }).then((unlisten) => {
      unlistenActionStatus = unlisten;
    });

    listenToRunPhaseChanged((event) => {
      if (event.branchId === branchId) {
        runPhases.set(event.executionId, event.phase);
        runPhases = new Map(runPhases);
      }
    }).then((unlisten) => {
      unlistenRunPhaseChanged = unlisten;
    });

    return () => {
      unlistenActionStatus?.();
      unlistenRunPhaseChanged?.();
    };
  });

  onMount(() => {
    loadActions();
    loadRunningActions();
    getAvailableOpeners().then((apps) => (openerApps = apps));
    window.addEventListener('project-actions-changed', handleActionsChanged as EventListener);

    listenToRepoActionsDetection((event) => {
      if (!event.detecting) {
        loadActions();
      }
    }).then((unlisten) => {
      unlistenRepoActionsDetection = unlisten;
    });

    window.addEventListener('keydown', handleAltDown);
    window.addEventListener('keyup', handleAltUp);
  });

  onDestroy(() => {
    unlistenActionStatus?.();
    unlistenRunPhaseChanged?.();
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

  let isInitializing = $derived(
    (isLocal && !branch.worktreePath && !worktreeError) ||
      (isRemote && remoteWorkspaceStatus === 'starting')
  );

  let primaryRunAction = $derived.by(() => {
    return getPrimaryRunAction(groupedActions);
  });

  let remainingRunActions = $derived.by(() => {
    return getRemainingRunActions(groupedActions);
  });

  let hasActionsForSubmenu = $derived.by(() => {
    const actionTypes = ['run', 'build', 'format', 'check', 'test', 'cleanUp', 'prerun'] as const;
    return actionTypes.some((type) => {
      const typeActions = type === 'run' ? remainingRunActions : groupedActions[type];
      return typeActions && typeActions.length > 0;
    });
  });

  function handleActionsSubmenuEnter() {
    if (actionsSubmenuTimeout) {
      clearTimeout(actionsSubmenuTimeout);
      actionsSubmenuTimeout = null;
    }
    showActionsSubmenu = true;
  }

  function handleActionsSubmenuLeave() {
    actionsSubmenuTimeout = setTimeout(() => {
      showActionsSubmenu = false;
      actionsSubmenuTimeout = null;
    }, 100);
  }

  function handleOpenInSubmenuEnter() {
    if (openInSubmenuTimeout) {
      clearTimeout(openInSubmenuTimeout);
      openInSubmenuTimeout = null;
    }
    showOpenInSubmenu = true;
  }

  function handleOpenInSubmenuLeave() {
    openInSubmenuTimeout = setTimeout(() => {
      showOpenInSubmenu = false;
      openInSubmenuTimeout = null;
    }, 100);
  }

  async function handleOpenInApp(appId: string) {
    showMoreMenu = false;
    showOpenInSubmenu = false;
    if (branch.worktreePath) {
      await openInApp(branch.worktreePath, appId);
    }
  }

  async function handleCopyPath() {
    showMoreMenu = false;
    showOpenInSubmenu = false;
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
    showMoreMenu = false;

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
        actionName: action.name,
        isStopping: stoppingExecutions.has(existingExecution.executionId),
      };
      return;
    }

    try {
      await runBranchAction(branch.id, action.id);
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
      actionName: execution.actionName,
      isStopping: stoppingExecutions.has(execution.executionId),
    };
  }

  export function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest('.more-menu-container')) {
      showMoreMenu = false;
    }
  }

  function toggleMoreMenu(e: MouseEvent) {
    e.stopPropagation();
    showMoreMenu = !showMoreMenu;
  }

  function handleDeleteFromMenu() {
    showMoreMenu = false;
    onDelete?.();
  }

  function handleRenameFromMenu() {
    showMoreMenu = false;
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
      <button
        class="running-action-button"
        class:running={isRunning}
        class:stopping={isStopping}
        class:completed={execution.status === 'completed'}
        class:failed={execution.status === 'failed'}
        class:show-stop={showStopIcon}
        onclick={() => {
          if (isRunning && altHeld && !isStopping) {
            handleStopAction(execution.executionId, execution.actionName);
          } else {
            handleShowActionOutput(execution);
          }
        }}
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
      </button>
    </div>
  {/each}
  <!-- Primary run action button -->
  {#if !isInitializing && primaryRunAction}
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
          <button
            class="primary-action-pill-main"
            class:stopping={isStopping}
            class:show-stop={showStopIcon}
            onclick={() => {
              if (altHeld && !isStopping && execution) {
                handleStopAction(execution.executionId, primaryRunAction.name);
              } else if (execution) {
                handleShowActionOutput(execution);
              }
            }}
            title={isStopping
              ? 'Stopping…'
              : showStopIcon
                ? `Stop ${primaryRunAction.name}`
                : `View output for ${primaryRunAction.name}`}
          >
            {#if isStopping}
              <Spinner size={14} class="danger" />
            {:else if showStopIcon}
              <StopCircle size={14} />
            {:else}
              <SineWave size={14} />
            {/if}
          </button>
          <button
            class="primary-action-pill-copy"
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
            title="Copy endpoint: {copyUrl}"
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
          </button>
        </div>
      {:else}
        <!-- Standard circular button -->
        <button
          class="primary-action-button"
          class:running={isRunning}
          class:stopping={isStopping}
          class:completed={execution?.status === 'completed'}
          class:failed={execution?.status === 'failed'}
          class:show-stop={showStopIcon}
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
        </button>
      {/if}
    </div>
  {/if}
{/if}
<div class="more-menu-container">
  <button class="more-button" onclick={toggleMoreMenu} title="More options">
    <MoreVertical size={16} />
  </button>
  {#if showMoreMenu}
    <div class="more-menu">
      {#if !isInitializing}
        <!-- Remote-only: Copy workspace name -->
        {#if isRemote && branch.workspaceName}
          <button
            class="more-menu-item"
            onclick={() => {
              showMoreMenu = false;
              navigator.clipboard.writeText(branch.workspaceName!);
            }}
          >
            <Copy size={14} />
            Copy Workspace Name
          </button>
        {/if}

        <!-- Actions submenu -->
        {#if hasActionsForSubmenu}
          <div class="submenu-container">
            <button
              class="more-menu-item submenu-trigger"
              onmouseenter={handleActionsSubmenuEnter}
              onmouseleave={handleActionsSubmenuLeave}
            >
              <Play size={14} />
              Actions
              <ChevronDown size={12} class="submenu-chevron" />
            </button>
            {#if showActionsSubmenu}
              <div
                class="submenu"
                role="group"
                onmouseenter={handleActionsSubmenuEnter}
                onmouseleave={handleActionsSubmenuLeave}
              >
                {#each ['run', 'build', 'format', 'check', 'test', 'cleanUp', 'prerun'] as type}
                  {@const typeActions = type === 'run' ? remainingRunActions : groupedActions[type]}
                  {#if typeActions.length > 0}
                    {#each typeActions as action (action.id)}
                      {@const Icon = getActionIcon(type)}
                      <button
                        class="more-menu-item action-item"
                        onclick={() => handleRunAction(action)}
                      >
                        <Icon size={14} />
                        {action.name}
                      </button>
                    {/each}
                  {/if}
                {/each}
              </div>
            {/if}
          </div>
        {/if}

        <!-- Local-only: Open In submenu -->
        {#if isLocal && branch.worktreePath && openerApps.length > 0}
          <div class="menu-separator"></div>
          <div class="submenu-container">
            <button
              class="more-menu-item submenu-trigger"
              onmouseenter={handleOpenInSubmenuEnter}
              onmouseleave={handleOpenInSubmenuLeave}
            >
              <ExternalLink size={14} />
              Open In
              <ChevronDown size={12} class="submenu-chevron" />
            </button>
            {#if showOpenInSubmenu}
              <div
                class="submenu"
                role="group"
                onmouseenter={handleOpenInSubmenuEnter}
                onmouseleave={handleOpenInSubmenuLeave}
              >
                {#each openerApps as app (app.id)}
                  <button class="more-menu-item" onclick={() => handleOpenInApp(app.id)}>
                    {app.name}
                  </button>
                {/each}
                <div class="menu-separator"></div>
                <button class="more-menu-item" onclick={handleCopyPath}>
                  <Copy size={14} />
                  Copy Path
                </button>
              </div>
            {/if}
          </div>
        {:else if isLocal && branch.worktreePath}
          <div class="menu-separator"></div>
          <button class="more-menu-item" onclick={handleCopyPath}>
            <Copy size={14} />
            Copy Worktree Path
          </button>
        {/if}

        <div class="menu-separator"></div>
        <button class="more-menu-item" onclick={handleRenameFromMenu}>
          <GitBranch size={14} />
          Rename Branch
        </button>
          <button
            class="more-menu-item"
            disabled={newCommitDisabled}
            onclick={() => {
              showMoreMenu = false;
              onRebaseBranch?.();
            }}
          >
          <GitBranch size={14} />
          Rebase Branch
        </button>
        {#if commitCount >= 2}
          <button
            class="more-menu-item"
            disabled={newCommitDisabled}
            onclick={() => {
              showMoreMenu = false;
              onSquashCommits?.();
            }}
          >
            <GitBranch size={14} />
            Squash Commits
          </button>
        {/if}
        <div class="menu-separator"></div>
      {/if}
      <button class="more-menu-item danger" onclick={handleDeleteFromMenu}>
        <Trash2 size={14} />
        Delete Repo
      </button>
    </div>
  {/if}
</div>

{#if actionOutputModal}
  <ActionOutputModal
    executionId={actionOutputModal.executionId}
    branchId={branch.id}
    actionName={actionOutputModal.actionName}
    isStopping={actionOutputModal.isStopping}
    onClose={() => (actionOutputModal = null)}
    {onNoteCreated}
  />
{/if}

<style>
  /* More menu */
  .more-menu-container {
    position: relative;
  }

  .more-button {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-faint);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .more-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .more-menu {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: visible;
    z-index: 100;
    min-width: 160px;
  }

  .more-menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    text-align: left;
  }

  .more-menu-item:hover {
    background-color: var(--bg-hover);
  }

  .more-menu-item :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .more-menu-item:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .more-menu-item:disabled:hover {
    background-color: transparent;
  }

  .more-menu-item.danger:hover {
    background-color: var(--ui-danger-bg);
    color: var(--ui-danger);
  }

  .more-menu-item.danger:hover :global(svg) {
    color: var(--ui-danger);
  }

  .menu-separator {
    height: 1px;
    background-color: var(--border-subtle);
    margin: 4px 0;
  }

  /* Submenu styles */
  .submenu-container {
    position: relative;
  }

  .submenu-trigger {
    justify-content: space-between;
  }

  .submenu-trigger :global(.submenu-chevron) {
    margin-left: auto;
    transform: rotate(-90deg);
  }

  .submenu {
    position: absolute;
    left: 100%;
    top: 0;
    margin-left: 2px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    max-height: 400px;
    z-index: 101;
    min-width: 160px;
    display: flex;
    flex-direction: column;
  }

  /* Enable scrolling for actions submenu */
  .more-menu > .submenu-container > .submenu {
    overflow-y: auto;
    overflow-x: visible;
  }

  /* Primary action button — circular icon-only */
  .primary-action-container {
    display: flex;
    align-items: center;
    overflow: hidden;
  }

  .primary-action-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: var(--bg-elevated);
    border: none;
    border-radius: 50%;
    color: var(--text-base);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .primary-action-button:hover {
    background: var(--bg-hover);
  }

  .primary-action-button.running {
    background: var(--bg-hover);
    color: var(--text-muted);
  }

  .primary-action-button.running:hover {
    background: var(--bg-elevated);
  }

  .primary-action-button.completed {
    background: var(--bg-hover);
    color: var(--status-added);
  }

  .primary-action-button.failed {
    background: var(--bg-hover);
    color: var(--ui-danger);
  }

  .primary-action-button.stopping {
    opacity: 0.6;
    cursor: pointer;
  }

  .primary-action-button.stopping:hover {
    background: var(--bg-elevated);
  }

  .primary-action-button.show-stop {
    color: var(--ui-danger);
  }

  .primary-action-button :global(svg) {
    flex-shrink: 0;
    width: 14px;
    height: 14px;
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

  .primary-action-pill-main {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .primary-action-pill-main:hover {
    color: var(--text-base);
  }

  .primary-action-pill-main.stopping {
    opacity: 0.6;
  }

  .primary-action-pill-main.show-stop {
    color: var(--ui-danger);
  }

  .primary-action-pill-main :global(svg) {
    flex-shrink: 0;
    width: 14px;
    height: 14px;
  }

  .primary-action-pill-copy {
    display: flex;
    align-items: center;
    justify-content: center;
    position: relative;
    width: 28px;
    height: 28px;
    padding: 0;
    background: none;
    border: none;
    border-left: 1px solid var(--border-muted);
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .copy-icon-wrapper {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .primary-action-pill-copy:hover {
    color: var(--text-base);
    background: var(--bg-elevated);
  }

  .primary-action-pill-copy :global(svg) {
    flex-shrink: 0;
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

  .running-action-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    white-space: nowrap;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  .running-action-button:hover {
    background: var(--bg-hover);
    border-color: var(--border-focus);
  }

  .running-action-button.completed {
    border-color: var(--status-added);
    color: var(--status-added);
  }

  .running-action-button.failed {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .running-action-button.stopping {
    opacity: 0.6;
    cursor: pointer;
  }

  .running-action-button.stopping:hover {
    background: var(--bg-elevated);
    border-color: var(--border-muted);
  }

  .running-action-button.show-stop {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .running-action-button :global(svg) {
    flex-shrink: 0;
  }
</style>
