import { describe, expect, it } from 'vitest';
import type { DoctorCheck } from '../../api/commands';
import {
  canOfferLogin,
  doctorCheckForProvider,
  isAuthCodePrompt,
  isAuthenticationError,
} from './authRecovery';

function check(overrides: Partial<DoctorCheck> = {}): DoctorCheck {
  return {
    id: 'ai-agent-claude',
    label: 'Claude Code',
    status: 'warn',
    message: 'Installed, not authenticated',
    fixUrl: null,
    fixCommand: 'claude-agent-acp --cli auth login',
    fixType: 'auth',
    path: '/usr/local/bin/claude-agent-acp',
    bridgePath: null,
    rawOutput: null,
    authStatus: 'notAuthenticated',
    installedVersion: null,
    latestVersion: null,
    updateAvailable: null,
    installSource: null,
    selfUpdating: null,
    main: null,
    bridge: null,
    ...overrides,
  };
}

describe('authentication recovery helpers', () => {
  it.each([
    'ACP protocol failed: OAuth token has expired; authentication required',
    'Error: missing CODEX_API_KEY (or OPENAI_API_KEY)',
    'nested ACP error: Unauthorized (401)',
  ])('recognizes authentication error: %s', (message) => {
    expect(isAuthenticationError(message)).toBe(true);
  });

  it('does not turn unrelated failures into authentication actions', () => {
    expect(isAuthenticationError('ACP protocol failed: connection refused')).toBe(false);
    expect(isAuthenticationError('npm install failed with exit code 1')).toBe(false);
  });

  it('only offers login for a positively detected signed-out agent', () => {
    expect(canOfferLogin(check())).toBe(true);
    expect(canOfferLogin(check({ authStatus: 'unknown' }))).toBe(false);
    expect(canOfferLogin(check({ authStatus: 'authenticated' }))).toBe(false);
    expect(canOfferLogin(check({ fixType: null }))).toBe(false);
  });

  it('matches a session provider to the existing doctor report', () => {
    const report = { checks: [check(), check({ id: 'ai-agent-codex', label: 'Codex' })] };
    expect(doctorCheckForProvider('codex', report)?.label).toBe('Codex');
    expect(doctorCheckForProvider('pi', report)).toBeNull();
    expect(doctorCheckForProvider(null, report)).toBeNull();
  });

  it.each(['Enter authentication code:', 'Paste the code here', 'input your token'])(
    'recognizes a code prompt: %s',
    (line) => {
      expect(isAuthCodePrompt(line)).toBe(true);
    }
  );
});
