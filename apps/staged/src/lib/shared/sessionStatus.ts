export function isSessionActive(status: string | null): boolean {
  return status === 'queued' || status === 'running';
}
