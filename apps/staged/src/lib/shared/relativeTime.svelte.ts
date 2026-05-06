import {
  formatRelativeTime as formatRelativeTimeBase,
  formatRelativeTimeSeconds as formatRelativeTimeSecondsBase,
} from './relativeTime';

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

class SecondNowStore {
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

    setInterval(() => {
      this.value = Date.now();
    }, 1000);
  }
}

export const minuteNow = new MinuteNowStore();
export const secondNow = new SecondNowStore();

export function formatRelativeTime(timestampMs: number, nowMs = minuteNow.now()): string {
  return formatRelativeTimeBase(timestampMs, nowMs);
}

export function formatRelativeTimeSeconds(
  timestampSeconds: number,
  nowMs = minuteNow.now()
): string {
  return formatRelativeTimeSecondsBase(timestampSeconds, nowMs);
}
