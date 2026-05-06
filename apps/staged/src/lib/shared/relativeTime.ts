function relativeParts(timestampMs: number, nowMs: number) {
  const date = new Date(timestampMs);
  const diffMs = Math.max(0, nowMs - date.getTime());
  const diffSecs = Math.floor(diffMs / 1000);
  const diffMins = Math.floor(diffSecs / 60);
  const diffHours = Math.floor(diffMins / 60);
  const diffDays = Math.floor(diffHours / 24);

  return { date, diffSecs, diffMins, diffHours, diffDays };
}

export function formatRelativeTime(timestampMs: number, nowMs = Date.now()): string {
  const { date, diffMins, diffHours, diffDays } = relativeParts(timestampMs, nowMs);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

export function formatPreciseRelativeTime(timestampMs: number, nowMs = Date.now()): string {
  const { date, diffSecs, diffMins, diffHours, diffDays } = relativeParts(timestampMs, nowMs);

  if (diffSecs < 60) return `${diffSecs}s ago`;
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

export function formatRelativeTimeSeconds(timestampSeconds: number, nowMs = Date.now()): string {
  return formatRelativeTime(timestampSeconds * 1000, nowMs);
}
