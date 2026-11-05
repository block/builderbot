import { describe, expect, it } from 'vitest';
import {
  COMPOSER_DISCONNECTED_TITLE,
  COMPOSER_LOADING_TITLE,
  COMPOSER_SEND_MODE_OPTIONS,
  COMPOSER_UNAVAILABLE_TITLE,
  composerSendControl,
  composerSessionBlock,
  composerSubmitBlock,
  composerSubmitBlockTitle,
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

describe('composerSessionBlock', () => {
  it('reports loading while the fetch is in flight, with or without a stale row', () => {
    expect(composerSessionBlock({ loading: true, hasSession: false })).toBe('loading');
    expect(composerSessionBlock({ loading: true, hasSession: true })).toBe('loading');
  });

  it('reports unavailable once the fetch settled without a row', () => {
    expect(composerSessionBlock({ loading: false, hasSession: false })).toBe('unavailable');
  });

  it('reports no block once the row is in', () => {
    expect(composerSessionBlock({ loading: false, hasSession: true })).toBeNull();
  });

  it('blocks a disconnected session, behind loading and a missing row', () => {
    expect(composerSessionBlock({ loading: false, hasSession: true, disconnected: true })).toBe(
      'disconnected'
    );
    expect(composerSessionBlock({ loading: true, hasSession: true, disconnected: true })).toBe(
      'loading'
    );
    expect(composerSessionBlock({ loading: false, hasSession: false, disconnected: true })).toBe(
      'unavailable'
    );
  });
});

describe('composerSubmitBlock', () => {
  it('disables submit while the session row is loading, whatever the draft holds', () => {
    const sessionBlock = 'loading';
    expect(composerSubmitBlock({ sessionBlock, sending: false, hasText: true })).toBe('loading');
    expect(composerSubmitBlock({ sessionBlock, sending: false, hasText: false })).toBe('loading');
    // Loading explains the block better than a spinner that is not spinning.
    expect(composerSubmitBlock({ sessionBlock, sending: true, hasText: true })).toBe('loading');
  });

  it('disables submit once the load settled without a row', () => {
    const sessionBlock = 'unavailable';
    expect(composerSubmitBlock({ sessionBlock, sending: false, hasText: true })).toBe(
      'unavailable'
    );
    expect(composerSubmitBlock({ sessionBlock, sending: true, hasText: false })).toBe(
      'unavailable'
    );
  });

  it('disables submit while a send is in flight', () => {
    expect(composerSubmitBlock({ sessionBlock: null, sending: true, hasText: true })).toBe(
      'sending'
    );
    expect(composerSubmitBlock({ sessionBlock: null, sending: true, hasText: false })).toBe(
      'sending'
    );
  });

  it('disables submit on an empty draft and otherwise allows it', () => {
    expect(composerSubmitBlock({ sessionBlock: null, sending: false, hasText: false })).toBe(
      'empty'
    );
    expect(composerSubmitBlock({ sessionBlock: null, sending: false, hasText: true })).toBeNull();
  });
});

describe('composerSubmitBlockTitle', () => {
  it('titles the session blocks differently so a settled failure reads as such', () => {
    expect(composerSubmitBlockTitle('loading')).toBe(COMPOSER_LOADING_TITLE);
    expect(composerSubmitBlockTitle('unavailable')).toBe(COMPOSER_UNAVAILABLE_TITLE);
    expect(composerSubmitBlockTitle('disconnected')).toBe(COMPOSER_DISCONNECTED_TITLE);
    expect(COMPOSER_DISCONNECTED_TITLE).toContain('Stop it to send a follow-up');
    expect(COMPOSER_LOADING_TITLE).not.toBe(COMPOSER_UNAVAILABLE_TITLE);
  });

  it('leaves the control title alone for blocks the control already explains', () => {
    expect(composerSubmitBlockTitle('sending')).toBeNull();
    expect(composerSubmitBlockTitle('empty')).toBeNull();
    expect(composerSubmitBlockTitle(null)).toBeNull();
  });
});
