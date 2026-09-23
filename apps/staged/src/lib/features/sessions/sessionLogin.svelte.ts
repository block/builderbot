/**
 * Pane-local orchestration over the shared agent login. Create once during
 * component initialization; the effects follow the pane's reactive context.
 *
 * The shared Doctor controller owns run identity, output, code submission,
 * cancellation and reconnect recovery. A login deliberately outlives the pane:
 * closing or switching sessions must not tear it down, and its completion must
 * still refresh the global doctor report. authRecovery.ts stays pure.
 */
import type { Session } from '../../types';
import {
  agentLogin,
  attachAgentLogin,
  startAgentLogin,
  type AgentLoginOutcome,
} from '../doctor/agentLogin.svelte';
import { doctorState, runChecks } from '../doctor/doctor.svelte';
import { canOfferLogin, doctorCheckForProvider, isAuthenticationError } from './authRecovery';

export function createSessionLoginController(context: {
  getActive: () => boolean;
  getSessionId: () => string;
  getSession: () => Pick<Session, 'provider' | 'status' | 'errorMessage'> | null;
}) {
  // A failed scan must not be retried on every flush or reopen. Unlike the
  // report request, an attach probe is allowed again on the next open.
  let reportRequestedFor: string | null = null;
  let attachRequestedFor: string | null = null;

  function checkId(): string | null {
    const provider = context.getSession()?.provider;
    return provider ? `ai-agent-${provider}` : null;
  }

  function canLogin(): boolean {
    return canOfferLogin(
      doctorCheckForProvider(context.getSession()?.provider, doctorState.report)
    );
  }

  function hasAuthFailure(): boolean {
    const session = context.getSession();
    return (
      (session?.status === 'error' || session?.status === 'cancelled') &&
      isAuthenticationError(session.errorMessage)
    );
  }

  function refreshAfterLogin(outcome: AgentLoginOutcome | null) {
    // "Completed" can also mean a reconnect found the run had ended, not
    // necessarily that sign-in succeeded. Doctor's probe decides that.
    if (outcome === 'completed') void runChecks();
  }

  $effect(() => {
    const id = context.getSessionId();
    if (!context.getActive() || !id || !hasAuthFailure()) return;
    // On a fresh launch the Doctor panel may never have populated the report
    // that determines whether this provider can offer Log in.
    if (doctorState.report || doctorState.loading || reportRequestedFor === id) return;
    reportRequestedFor = id;
    void runChecks();
  });

  $effect(() => {
    const id = context.getSessionId();
    const check = checkId();
    if (!context.getActive()) {
      attachRequestedFor = null;
      return;
    }
    if (!id || !check || !hasAuthFailure()) return;
    if (agentLogin.running || attachRequestedFor === id) return;
    attachRequestedFor = id;
    // Probe even without a report or an eligible login command: another pane,
    // client or webview may already have started a login we can follow.
    void attachAgentLogin(check)
      .then(refreshAfterLogin)
      .catch(() => {
        // Login failures are rendered by AgentLoginPrompt from the shared record.
      });
  });

  return {
    get checkId() {
      return checkId();
    },
    get canLogin() {
      return canLogin();
    },
    get running() {
      return agentLogin.running && agentLogin.checkId === checkId();
    },
    async start(): Promise<void> {
      const check = checkId();
      // Presentation is provider-local, but any running login owns the one
      // shared record and prevents a new start for another provider.
      if (!check || !canLogin() || agentLogin.running) return;
      try {
        refreshAfterLogin(await startAgentLogin(check));
      } catch {
        // Login failures are rendered by AgentLoginPrompt from the shared record.
      }
    },
  };
}
