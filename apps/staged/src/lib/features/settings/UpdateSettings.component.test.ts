// @vitest-environment node
/**
 * Component tests for the Settings updater surface (UpdateSettings.svelte).
 *
 * Rendered with svelte/server so every major updater state can be asserted
 * against the markup — status text, accessible button text, busy/disabled
 * states, release notes, and the error/retry affordance — without a browser.
 */
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import UpdateSettings from './UpdateSettings.svelte';
import { initialUpdaterState, type UpdaterState } from '../../services/updater';

function renderUpdateSettings(overrides: Partial<UpdaterState> = {}, appVersion = '0.1.9') {
  const updater: UpdaterState = { ...initialUpdaterState(), ...overrides };
  const { body } = render(UpdateSettings, {
    props: { updater, appVersion, onCheck: () => {} },
  });
  return body;
}

/** Whether the rendered check/retry button carries the `disabled` attribute. */
function buttonDisabled(html: string): boolean {
  const match = html.match(/<button\b[^>]*>/);
  if (!match) throw new Error('expected a rendered <button>');
  // Attribute (` disabled` / ` disabled=""`), not the Tailwind `disabled:` variants.
  return /\sdisabled(?:=""|(?=[\s>]))/.test(match[0]);
}

/** Text content of the rendered markup, with tags stripped and whitespace collapsed. */
function textOf(html: string): string {
  return html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&#\d+;|&[a-z]+;/g, (entity) => {
      if (entity === '&amp;') return '&';
      if (entity === '&lt;') return '<';
      if (entity === '&gt;') return '>';
      if (entity === '&#8230;' || entity === '&hellip;') return '…';
      return entity;
    })
    .replace(/\s+/g, ' ')
    .trim();
}

describe('UpdateSettings', () => {
  it('shows the installed version', () => {
    const text = textOf(renderUpdateSettings({}, '1.2.3'));
    expect(text).toContain('Staged v1.2.3');
  });

  it('renders an accessible live status region', () => {
    const html = renderUpdateSettings();
    expect(html).toContain('role="status"');
    expect(html).toContain('aria-live="polite"');
  });

  describe('unavailable', () => {
    it('explains that this build never updates and hides the check button', () => {
      const html = renderUpdateSettings({ status: 'unavailable' });
      const text = textOf(html);
      expect(text).toContain('Automatic updates are not available in this build.');
      expect(text).toContain('Automatic updates only run in official release builds.');
      expect(html).not.toContain('<button');
    });
  });

  describe('idle', () => {
    it('offers a check-for-updates action', () => {
      const html = renderUpdateSettings({ status: 'idle' });
      const text = textOf(html);
      expect(text).toContain('Updates are checked automatically in the background.');
      expect(text).toContain('Check for updates');
      expect(html).toContain('<button');
      expect(buttonDisabled(html)).toBe(false);
    });
  });

  describe('checking', () => {
    it('shows the in-progress status and disables the button', () => {
      const html = renderUpdateSettings({ status: 'checking' });
      expect(textOf(html)).toContain('Checking for updates…');
      expect(buttonDisabled(html)).toBe(true);
      expect(html).toContain('aria-busy="true"');
    });
  });

  describe('up-to-date', () => {
    it('confirms the app is current and allows another check', () => {
      const html = renderUpdateSettings({ status: 'up-to-date' });
      const text = textOf(html);
      expect(text).toContain('Staged is up to date.');
      expect(text).toContain('Check for updates');
      expect(buttonDisabled(html)).toBe(false);
    });
  });

  describe('update-available', () => {
    it('shows the available version and release notes', () => {
      const html = renderUpdateSettings({
        status: 'update-available',
        availableVersion: '0.2.0',
        releaseNotes: 'Adds in-app update settings.',
      });
      const text = textOf(html);
      expect(text).toContain('Version 0.2.0 is available.');
      expect(text).toContain('Release notes');
      expect(text).toContain('Adds in-app update settings.');
      expect(text).toContain('Check for updates');
    });

    it('falls back to a generic message without a version', () => {
      const text = textOf(renderUpdateSettings({ status: 'update-available' }));
      expect(text).toContain('A new version is available.');
      expect(text).not.toContain('Release notes');
    });
  });

  describe('installing', () => {
    it('shows install progress with the version and disables the button', () => {
      const html = renderUpdateSettings({
        status: 'installing',
        availableVersion: '0.2.0',
        releaseNotes: 'Notes',
      });
      const text = textOf(html);
      expect(text).toContain('Downloading and installing version 0.2.0…');
      expect(text).toContain('Release notes');
      expect(buttonDisabled(html)).toBe(true);
      expect(html).toContain('aria-busy="true"');
    });
  });

  describe('restart-required', () => {
    it('announces the pending restart and disables the button', () => {
      const html = renderUpdateSettings({ status: 'restart-required' });
      expect(textOf(html)).toContain('Update installed. Staged will restart to finish.');
      expect(buttonDisabled(html)).toBe(true);
    });
  });

  describe('error', () => {
    it('shows the recoverable error with a retry action', () => {
      const html = renderUpdateSettings({ status: 'error', error: 'Update check timed out.' });
      const text = textOf(html);
      expect(text).toContain('Update check timed out.');
      expect(text).toContain('Try again');
      expect(html).toContain('update-status-error');
      expect(buttonDisabled(html)).toBe(false);
    });

    it('falls back to a generic error message', () => {
      const text = textOf(renderUpdateSettings({ status: 'error', error: null }));
      expect(text).toContain('The last update check failed.');
      expect(text).toContain('Try again');
    });
  });
});
