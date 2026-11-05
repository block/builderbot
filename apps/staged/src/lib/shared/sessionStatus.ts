/**
 * A session that holds its slot (and so keeps its branch busy): `running`, or
 * `disconnected` — a Blox session that lost its local connection mid-turn but
 * whose remote turn may still be running. Excludes `queued`.
 */
export function isSessionInFlight(status: string | null | undefined): boolean {
  return status === 'running' || status === 'disconnected';
}

export function isSessionActive(status: string | null | undefined): boolean {
  return status === 'queued' || isSessionInFlight(status);
}
