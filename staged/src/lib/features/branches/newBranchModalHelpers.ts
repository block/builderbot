export const WORKSPACE_NAME_MAX_LENGTH = 32;

export function sanitizeBranchName(title: string): string {
  return title
    .toLowerCase()
    .replace(/[\s_]+/g, '-')
    .replace(/[~^:?*\[\]\\@{}"'`!#$%&()|<>=+;,]/g, '')
    .replace(/[-.]+/g, '-')
    .replace(/^[-.]+|[-.]+$/g, '');
}

export function workspaceName(name: string): string {
  if (!name) return '';
  let fullName = `stg-${name}`;
  if (fullName.length > WORKSPACE_NAME_MAX_LENGTH) {
    fullName = fullName.slice(0, WORKSPACE_NAME_MAX_LENGTH).replace(/-+$/, '');
  }
  return fullName;
}

export function formatTimeAgo(dateStr: string): string {
  const date = new Date(dateStr);
  const now = Date.now();
  const diffMs = now - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  return `${diffDays}d ago`;
}

export function formatBranchName(name: string): string {
  return name.replace(/^origin\//, '');
}

export function repoName(path: string): string {
  return path.split('/').pop() || path;
}
