import { describe, expect, it } from 'vitest';
import type { DoctorCheck } from '../../api/commands';
import { canOfferLogin, doctorCheckForProvider, isAuthenticationError } from './authRecovery';

const CLAUDE_LOGIN = 'claude-agent-acp --cli auth login';

/** The Claude check as doctor reports a positively signed-out agent. */
function check(overrides: Partial<DoctorCheck> = {}): DoctorCheck {
  return {
    id: 'ai-agent-claude',
    label: 'Claude Code',
    status: 'warn',
    message: 'Installed, not authenticated',
    fixUrl: null,
    fixCommand: CLAUDE_LOGIN,
    fixType: 'auth',
    path: '/usr/local/bin/claude-agent-acp',
    bridgePath: null,
    rawOutput: null,
    authStatus: 'notAuthenticated',
    loginCommand: CLAUDE_LOGIN,
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

/**
 * The same check when the probe exited 0: passing, no fix attached, but the
 * static login command still present. This is what doctor reports for an
 * expired token — `auth status` never checks expiry — so it is the shape the
 * session alert sees in the most common real-world route into this UI.
 */
function passingCheck(overrides: Partial<DoctorCheck> = {}): DoctorCheck {
  return check({
    status: 'pass',
    message: 'Installed',
    fixCommand: null,
    fixType: null,
    authStatus: 'authenticated',
    ...overrides,
  });
}

/** A resolved provider that has no login command at all. */
function providerWithoutLogin(id: string, label: string): DoctorCheck {
  return passingCheck({
    id,
    label,
    path: `/usr/local/bin/${id.replace('ai-agent-', '')}`,
    authStatus: null,
    loginCommand: null,
  });
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

  describe('canOfferLogin', () => {
    it('offers login for a positively signed-out agent', () => {
      expect(canOfferLogin(check())).toBe(true);
    });

    it('offers login when the probe says authenticated but the session failed to authenticate', () => {
      // The expired-token case: the probe exits 0 on a credentials record the
      // vendor will reject, so the passing check must still be able to log in.
      expect(canOfferLogin(passingCheck())).toBe(true);
    });

    it('offers login when the provider has a login command but no status probe', () => {
      expect(canOfferLogin(passingCheck({ authStatus: 'notApplicable' }))).toBe(true);
    });

    it('does not rely on the fix fields, which a passing check leaves empty', () => {
      expect(canOfferLogin(passingCheck({ fixType: null, fixCommand: null }))).toBe(true);
      expect(canOfferLogin(check({ fixType: null, fixCommand: null }))).toBe(true);
    });

    it('withholds login when the probe could not run the binary', () => {
      // `unknown` means the binary was not on the login shell's PATH or the
      // probe never ran; a login through the same binary would fail the same way.
      expect(canOfferLogin(check({ authStatus: 'unknown' }))).toBe(false);
      expect(canOfferLogin(passingCheck({ authStatus: 'unknown' }))).toBe(false);
    });

    it('withholds login without a doctor check for the provider', () => {
      expect(canOfferLogin(null)).toBe(false);
      expect(canOfferLogin(undefined)).toBe(false);
    });

    it('withholds login for providers without a login command', () => {
      expect(canOfferLogin(providerWithoutLogin('ai-agent-pi', 'Pi'))).toBe(false);
      expect(canOfferLogin(providerWithoutLogin('ai-agent-goose', 'Goose'))).toBe(false);
      // A stale fix on a check whose provider lost its login command is not a
      // login either: the static capability is the only thing that qualifies.
      expect(canOfferLogin(check({ loginCommand: null }))).toBe(false);
    });
  });

  it('matches a session provider to the existing doctor report', () => {
    const report = { checks: [check(), check({ id: 'ai-agent-codex', label: 'Codex' })] };
    expect(doctorCheckForProvider('codex', report)?.label).toBe('Codex');
    expect(doctorCheckForProvider('pi', report)).toBeNull();
    expect(doctorCheckForProvider(null, report)).toBeNull();
  });
});
