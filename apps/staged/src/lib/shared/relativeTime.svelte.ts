class MinuteNowStore {
  private value = $state(Date.now());

  constructor() {
    if (typeof window !== 'undefined') {
      this.start();
    }
  }

  now(): number {
    return this.value;
  }

  private start(): void {
    this.value = Date.now();

    const msUntilNextMinute = 60000 - (this.value % 60000);
    setTimeout(() => {
      this.value = Date.now();
      setInterval(() => {
        this.value = Date.now();
      }, 60000);
    }, msUntilNextMinute);
  }
}

export const minuteNow = new MinuteNowStore();

export function formatRelativeTime(timestampMs: number, nowMs = minuteNow.now()): string {
  const date = new Date(timestampMs);
  const diffMs = nowMs - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMins / 60);
  const diffDays = Math.floor(diffHours / 24);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

export function formatRelativeTimeSeconds(
  timestampSeconds: number,
  nowMs = minuteNow.now()
): string {
  return formatRelativeTime(timestampSeconds * 1000, nowMs);
}
