import type { DoctorCheck, DoctorReport } from '../../api/commands';

/** Authentication failures commonly arrive wrapped in one or more ACP errors. */
export function isAuthenticationError(message: string | null | undefined): boolean {
  if (!message) return false;
  const text = message.toLowerCase();

  return (
    /authenticat(?:e|ion|ed|ing)/.test(text) ||
    /auth[_ -]?required/.test(text) ||
    /unauthori[sz]ed/.test(text) ||
    /oauth/.test(text) ||
    /(?:api[_ ]?key|access token|refresh token|credential).*(?:missing|invalid|expired|required)/.test(
      text
    ) ||
    /(?:missing|invalid|expired|required).*(?:api[_ ]?key|access token|refresh token|credential)/.test(
      text
    ) ||
    /\b(?:codex_api_key|openai_api_key)\b/.test(text)
  );
}

/** Find the doctor check for the provider recorded on a session. */
export function doctorCheckForProvider(
  provider: string | null | undefined,
  report: DoctorReport | null | undefined
): DoctorCheck | null {
  if (!provider || !report) return null;
  return report.checks.find((check) => check.id === `ai-agent-${provider}`) ?? null;
}

/**
 * Whether a login can be offered for a session whose live error is an
 * authentication failure. The caller has already classified that error with
 * `isAuthenticationError`; this only asks whether a login is worth running.
 *
 * Two things gate it. The provider must have a login command — a static
 * capability doctor reports whenever the binary resolved, so Pi and Goose never
 * get one. And the probe must not have come back `unknown`: that means the
 * binary was not on the login shell's `PATH` or the probe never ran, and a
 * login through the same binary would fail the same way.
 *
 * `authenticated` is deliberately *not* an exclusion. Doctor's probe is the
 * exit code of the provider's own status command, and for Claude that is 0
 * for an expired token with a dead refresh token, for an expired token with no
 * refresh token, and for a well-formed but bogus token — it checks that a
 * credentials record exists, never its expiry. A fresh authentication failure
 * from this session's own agent process outranks a probe that verifiably does
 * not look. Nothing about the user's sign-in state is stored or inferred here:
 * the live error, the static capability, and the probe used only to exclude
 * `unknown` are read and forgotten, and the vendor stays the sole authority.
 *
 * One `authenticated` shape is a known false positive: a login shell that
 * exports `ANTHROPIC_API_KEY`. The Claude CLI reports itself logged in on that
 * variable alone, whatever the OAuth state, and an OAuth login does nothing
 * about the exported key — the agent's next session starts with the same
 * environment and fails the same way, after a login that reported success. So
 * `Log in` is offered there and cannot help. It is accepted rather than
 * excluded because `authenticated` cannot be split: the probe's exit code is
 * all doctor reports, and it is the same 0 for the expired token this gate
 * exists for and for the exported key. The probe's own JSON does tell them
 * apart (`apiKeySource: ANTHROPIC_API_KEY`, `authMethod: api_key` when no
 * OAuth record exists at all), so a future fix would key on a doctor-side
 * credential-source field lifted from that output — not on re-excluding
 * `authenticated`, which would close the berd#99 route again.
 */
export function canOfferLogin(check: DoctorCheck | null | undefined): boolean {
  if (!check?.loginCommand) return false;
  return check.authStatus !== 'unknown';
}
