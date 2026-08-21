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
 * A login action is offered only when doctor's existing auth probe positively
 * identified a signed-out agent. Unknown status is deliberately not guessed:
 * a login command can be ineffective when the binary or its credentials could
 * not be detected.
 */
export function canOfferLogin(check: DoctorCheck | null | undefined): boolean {
  return check?.authStatus === 'notAuthenticated' && check.fixType === 'auth' && !!check.fixCommand;
}

export function isAuthCodePrompt(line: string): boolean {
  const text = line.toLowerCase();
  return (
    /\b(?:enter|paste|提供|input|type|write|submit)\b.{0,40}\b(?:code|token)\b/.test(text) ||
    /\b(?:code|token)\b.{0,40}\b(?:enter|paste|input|type|write|submit)\b/.test(text)
  );
}
