<script lang="ts">
  import { AlertCircle, CheckCircle2, Info, TriangleAlert, X } from 'lucide-svelte';
  import type { AlertTone } from './alerts.svelte';

  interface Props {
    tone?: AlertTone;
    title?: string;
    message: string;
    dismissible?: boolean;
    onDismiss?: () => void;
  }

  let { tone = 'info', title, message, dismissible = true, onDismiss }: Props = $props();

  const iconByTone = {
    error: AlertCircle,
    warning: TriangleAlert,
    success: CheckCircle2,
    info: Info,
  } as const;

  let Icon = $derived(iconByTone[tone]);
</script>

<div class="alert-card" class:error={tone === 'error'} class:warning={tone === 'warning'}>
  <div class="alert-icon">
    <Icon size={15} />
  </div>
  <div class="alert-content">
    {#if title}
      <div class="alert-title">{title}</div>
    {/if}
    <div class="alert-message">{message}</div>
  </div>
  {#if dismissible}
    <button
      class="alert-dismiss"
      type="button"
      aria-label="Dismiss notification"
      onclick={() => onDismiss?.()}
    >
      <X size={14} />
    </button>
  {/if}
</div>

<style>
  .alert-card {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    box-shadow: var(--shadow-elevated);
    color: var(--text-primary);
    backdrop-filter: blur(8px);
  }

  .alert-card.error {
    border-color: color-mix(in srgb, var(--ui-danger) 45%, var(--border-muted));
  }

  .alert-card.warning {
    border-color: color-mix(in srgb, #d29922 45%, var(--border-muted));
  }

  .alert-icon {
    flex-shrink: 0;
    color: var(--text-faint);
    margin-top: 1px;
  }

  .alert-card.error .alert-icon {
    color: var(--ui-danger);
  }

  .alert-card.warning .alert-icon {
    color: #d29922;
  }

  .alert-content {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
  }

  .alert-title {
    font-size: var(--size-sm);
    font-weight: 600;
    line-height: 1.2;
  }

  .alert-message {
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.35;
  }

  .alert-dismiss {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
    flex-shrink: 0;
  }

  .alert-dismiss:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }
</style>
