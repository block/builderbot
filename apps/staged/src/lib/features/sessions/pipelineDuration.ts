export function formatPipelineStepDuration(
  startedAt: number | null,
  completedAt: number | null,
  currentTime: number
): string {
  if (startedAt == null) return '';

  const end = completedAt ?? currentTime;
  const totalSeconds = Math.round(Math.max(0, end - startedAt) / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const parts: string[] = [];

  if (hours > 0) parts.push(`${hours}h`);
  if (minutes > 0) parts.push(`${minutes}m`);
  if (seconds > 0 || parts.length === 0) parts.push(`${seconds}s`);

  return parts.join(' ');
}
