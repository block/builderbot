export function isEmptyFailedReview(params: {
  sessionStatus: string | null;
  sessionId: string | null;
  title: string | null;
  totalCount: number;
}): boolean {
  const { sessionStatus, sessionId, title, totalCount } = params;
  const isRunning = sessionStatus === 'running';
  const isQueued = sessionStatus === 'queued';
  const hasTitle = !!title?.trim();
  return !isRunning && !isQueued && !!sessionId && totalCount === 0 && !hasTitle;
}
