import type { ProjectAction } from '../../api/commands';

const TEXT_EXTENSIONS = ['.txt', '.md', '.markdown', '.text', '.rst', '.org', '.adoc'];

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

export function extractPrUrl(messages: { content: string; role: string }[]): string | null {
  for (const msg of messages) {
    if (msg.role !== 'assistant' && msg.role !== 'tool_result') continue;
    const markerMatch = msg.content.match(/PR_URL:\s*(https?:\/\/\S+)/);
    if (markerMatch) {
      // Strip trailing markdown characters (*, ), ], etc.) from the URL
      return markerMatch[1].replace(/[\*\)\]]+$/, '');
    }
  }

  for (const msg of messages) {
    const ghMatch = msg.content.match(/https:\/\/github\.com\/[^/]+\/[^/]+\/pull\/\d+/);
    if (ghMatch) return ghMatch[0];
  }

  return null;
}

export function extractPrNumber(url: string): number | null {
  const match = url.match(/\/pull\/(\d+)/);
  return match ? parseInt(match[1], 10) : null;
}

export function isPushRejectedNonFastForward(
  messages: { content: string; role: string }[]
): boolean {
  return messages.some(
    (msg) =>
      (msg.role === 'assistant' || msg.role === 'tool_result') &&
      msg.content.includes('PUSH_REJECTED: NON_FAST_FORWARD')
  );
}

export function isTextFile(filePath: string): boolean {
  const lower = filePath.toLowerCase();
  return TEXT_EXTENSIONS.some((ext) => lower.endsWith(ext));
}

export function fileNameFromPath(filePath: string): string {
  const parts = filePath.split('/');
  const name = parts[parts.length - 1] || filePath;
  return name.replace(/\.[^.]+$/, '');
}
