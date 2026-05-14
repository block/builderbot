export function isSessionActive(status: string | null | undefined): boolean {
  return status === 'queued' || status === 'running';
}
