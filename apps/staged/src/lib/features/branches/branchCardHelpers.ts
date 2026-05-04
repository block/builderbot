import type { ProjectAction } from '../../api/commands';
import type { PipelineExecution } from '../../types';

const IMAGE_EXTENSIONS = ['.png', '.jpg', '.jpeg', '.gif', '.webp'];

export function groupActionsByType(actions: ProjectAction[]): Record<string, ProjectAction[]> {
  const groups: Record<string, ProjectAction[]> = {
    prerun: [],
    run: [],
    build: [],
    format: [],
    check: [],
    test: [],
    cleanUp: [],
  };

  for (const action of actions) {
    if (groups[action.actionType]) {
      groups[action.actionType].push(action);
    }
  }

  return groups;
}

export function getPrimaryRunAction(
  groupedActions: Record<string, ProjectAction[]>
): ProjectAction | null {
  return groupedActions.run?.[0] ?? null;
}

export function getRemainingRunActions(
  groupedActions: Record<string, ProjectAction[]>
): ProjectAction[] {
  return groupedActions.run?.slice(1) ?? [];
}

export function getPrimaryActionExecution<T extends { actionId: string }>(
  runningActions: T[],
  primaryRunActionId: string | null
): T | null {
  if (!primaryRunActionId) return null;
  return runningActions.find((a) => a.actionId === primaryRunActionId) ?? null;
}

export function getSecondaryRunningActions<T extends { actionId: string }>(
  runningActions: T[],
  primaryRunActionId: string | null
): T[] {
  if (!primaryRunActionId) return runningActions;
  return runningActions.filter((a) => a.actionId !== primaryRunActionId);
}

export function getActionTypeLabel(actionType: string): string {
  switch (actionType) {
    case 'prerun':
      return 'Prerun';
    case 'run':
      return 'Run';
    case 'build':
      return 'Build';
    case 'format':
      return 'Format';
    case 'check':
      return 'Check';
    case 'test':
      return 'Test';
    case 'cleanUp':
      return 'Clean Up';
    default:
      return 'Action';
  }
}

export function formatBaseBranch(baseBranch: string): string {
  return baseBranch.replace(/^origin\//, '');
}

function normalizePrUrl(candidate: string): string | null {
  const trimmed = candidate.trim().replace(/^[<`'"\[(]+|[>`'"\]),.?!;:]+$/g, '');
  const match = trimmed.match(
    /^https:\/\/github\.com\/([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)\/pull\/(\d+)(?:[/?#].*)?$/
  );

  if (!match) return null;

  return `https://github.com/${match[1]}/${match[2]}/pull/${match[3]}`;
}

export function extractPrUrl(messages: { content: string; role: string }[]): string | null {
  for (const msg of messages) {
    if (msg.role !== 'assistant' && msg.role !== 'tool_result') continue;
    const markerMatch = msg.content.match(/PR_URL:\s*(\S+)/);
    if (markerMatch) {
      const normalized = normalizePrUrl(markerMatch[1]);
      if (normalized) return normalized;
    }
  }

  for (const msg of messages) {
    const urlMatches = msg.content.match(/https?:\/\/\S+/g) ?? [];
    for (const url of urlMatches) {
      const normalized = normalizePrUrl(url);
      if (normalized) return normalized;
    }
  }

  return null;
}

export function extractPrNumber(url: string): number | null {
  const match = url.match(/\/pull\/(\d+)/);
  return match ? parseInt(match[1], 10) : null;
}

type MessageLike = { content: string; role: string };

export type CompletedPushOutcome = 'succeeded' | 'rejected_non_fast_forward';

function containsNonFastForwardPushMarker(content: string): boolean {
  const lowerContent = content.toLowerCase();
  return (
    content.includes('PUSH_REJECTED: NON_FAST_FORWARD') || lowerContent.includes('non-fast-forward')
  );
}

export function isPushRejectedNonFastForward(messages: MessageLike[]): boolean {
  return messages.some(
    (msg) =>
      (msg.role === 'assistant' || msg.role === 'tool_result') &&
      containsNonFastForwardPushMarker(msg.content)
  );
}

export function classifyPipelinePushCompletion(
  pipeline: PipelineExecution | null | undefined,
  messages?: MessageLike[]
): CompletedPushOutcome | null {
  if (!pipeline) return null;

  const hasNonFastForward = pipeline.steps.some(
    (step) =>
      step.status === 'failed' &&
      step.output !== null &&
      containsNonFastForwardPushMarker(step.output)
  );
  if (hasNonFastForward) {
    // If AI ran after the pipeline failure (e.g. a force-push that failed with
    // --force-with-lease and the error happened to contain "non-fast-forward"),
    // the AI handled recovery — don't classify as rejected. Only treat it as
    // rejected when no AI session ran (the pipeline aborted immediately).
    const aiRan = messages && messages.some((m) => m.role === 'assistant');
    if (aiRan) return 'succeeded';
    return 'rejected_non_fast_forward';
  }

  const allStepsPassedOrSkipped = pipeline.steps.every(
    (step) => step.status === 'succeeded' || step.status === 'skipped'
  );
  if (pipeline.completedWithoutAi || allStepsPassedOrSkipped) return 'succeeded';

  return null;
}

export function classifyCompletedPushSession(
  pipeline: PipelineExecution | null | undefined,
  messages: MessageLike[]
): CompletedPushOutcome {
  return (
    classifyPipelinePushCompletion(pipeline, messages) ??
    (isPushRejectedNonFastForward(messages) ? 'rejected_non_fast_forward' : 'succeeded')
  );
}

export function isImageFile(filePath: string): boolean {
  const lower = filePath.toLowerCase();
  return IMAGE_EXTENSIONS.some((ext) => lower.endsWith(ext));
}

export function isMaybeTextFile(filePath: string): boolean {
  return !isImageFile(filePath);
}

/**
 * Insert file paths into a textarea at the cursor position, preserving undo history.
 * Uses `document.execCommand('insertText')` which, while deprecated, is the simplest
 * way to get undo-friendly insertion in Chromium/Tauri webviews.
 */
export function insertFilePathsAtCursor(element: HTMLElement, paths: string[]): void {
  const insert = paths.join('\n');
  element.focus();
  document.execCommand('insertText', false, insert);
}

export function fileNameFromPath(filePath: string): string {
  const parts = filePath.split('/');
  const name = parts[parts.length - 1] || filePath;
  return name.replace(/\.[^.]+$/, '');
}
