import { describe, expect, it } from 'vitest';
import {
  COMPOSER_SEND_MODE_OPTIONS,
  composerSendControl,
  normalizeComposerSendMode,
} from './composerSendControl';

describe('composerSendControl', () => {
  it('shows a single Send button that starts a turn when the session is not running', () => {
    for (const mode of ['queue', 'send'] as const) {
      const control = composerSendControl({ isLive: false, canSendLive: true, mode });
      expect(control).toMatchObject({ split: false, action: 'start', label: 'Send' });
    }
    expect(composerSendControl({ isLive: false, canSendLive: false, mode: 'queue' })).toMatchObject(
      { split: false, action: 'start', label: 'Send' }
    );
  });

  it('shows a single Queue button when the running session cannot take a message now', () => {
    for (const mode of ['queue', 'send'] as const) {
      const control = composerSendControl({ isLive: true, canSendLive: false, mode });
      expect(control).toMatchObject({ split: false, action: 'queue', label: 'Queue' });
    }
  });

  it('splits the button when live delivery is supported, with the primary following the mode', () => {
    expect(composerSendControl({ isLive: true, canSendLive: true, mode: 'queue' })).toMatchObject({
      split: true,
      action: 'queue',
      label: 'Queue',
    });
    expect(composerSendControl({ isLive: true, canSendLive: true, mode: 'send' })).toMatchObject({
      split: true,
      action: 'send-now',
      label: 'Send',
    });
  });

  it('gives each state a distinct placeholder', () => {
    const start = composerSendControl({ isLive: false, canSendLive: false, mode: 'queue' });
    const queue = composerSendControl({ isLive: true, canSendLive: true, mode: 'queue' });
    const sendNow = composerSendControl({ isLive: true, canSendLive: true, mode: 'send' });
    expect(new Set([start.placeholder, queue.placeholder, sendNow.placeholder]).size).toBe(3);
  });

  it('normalizes stored modes, defaulting to queue', () => {
    expect(normalizeComposerSendMode('send')).toBe('send');
    expect(normalizeComposerSendMode('queue')).toBe('queue');
    expect(normalizeComposerSendMode(undefined)).toBe('queue');
    expect(normalizeComposerSendMode('later')).toBe('queue');
    expect(normalizeComposerSendMode({ mode: 'send' })).toBe('queue');
  });

  it('offers exactly the two modes in the dropdown', () => {
    expect(COMPOSER_SEND_MODE_OPTIONS.map((o) => o.mode)).toEqual(['queue', 'send']);
  });
});
