export type AlertTone = 'error' | 'warning' | 'success' | 'info';

export interface AlertToast {
  id: string;
  tone: AlertTone;
  title?: string;
  message: string;
  dismissible: boolean;
}

interface ShowAlertOptions {
  tone?: AlertTone;
  title?: string;
  message: string;
  dismissible?: boolean;
  durationMs?: number;
}

const DEFAULT_DURATION_MS = 8000;
const MAX_VISIBLE_TOASTS = 4;

class AlertsStore {
  toasts = $state<AlertToast[]>([]);
  private timers = new Map<string, ReturnType<typeof setTimeout>>();

  show(options: ShowAlertOptions): string {
    const id = this.nextId();
    const toast: AlertToast = {
      id,
      tone: options.tone ?? 'info',
      title: options.title,
      message: options.message,
      dismissible: options.dismissible ?? true,
    };

    this.toasts = [...this.toasts, toast];
    if (this.toasts.length > MAX_VISIBLE_TOASTS) {
      const oldest = this.toasts[0];
      this.dismiss(oldest.id);
    }

    const durationMs = options.durationMs ?? DEFAULT_DURATION_MS;
    if (durationMs > 0) {
      const timer = setTimeout(() => this.dismiss(id), durationMs);
      this.timers.set(id, timer);
    }

    return id;
  }

  error(message: string, title = 'Error', durationMs?: number): string {
    return this.show({ tone: 'error', title, message, durationMs });
  }

  dismiss(id: string): void {
    const timer = this.timers.get(id);
    if (timer) {
      clearTimeout(timer);
      this.timers.delete(id);
    }
    this.toasts = this.toasts.filter((toast) => toast.id !== id);
  }

  clear(): void {
    for (const timer of this.timers.values()) {
      clearTimeout(timer);
    }
    this.timers.clear();
    this.toasts = [];
  }

  private nextId(): string {
    if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
      return crypto.randomUUID();
    }
    return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }
}

export const alerts = new AlertsStore();
