/**
 * Human wording for `Session.startedBy`, the control-server actor recorded
 * on a session that stagedctl started rather than the user.
 *
 * The stored value is a stable identifier (`session:<id>` or
 * `app:<grant-id>`), never a display name, so the label describes the kind
 * of actor and shows a short prefix of the id for cross-referencing. Unknown
 * shapes still render — the backend owns the vocabulary and a new prefix must
 * not hide the fact that the user did not start the session.
 */

const SHORT_ID_LENGTH = 8;

export function startedByLabel(startedBy: string | null | undefined): string | null {
  if (!startedBy) return null;
  const separator = startedBy.indexOf(':');
  if (separator <= 0 || separator === startedBy.length - 1) {
    return `Started by ${startedBy}`;
  }
  const kind = startedBy.slice(0, separator);
  const id = startedBy.slice(separator + 1);
  const shortId = id.length > SHORT_ID_LENGTH ? `${id.slice(0, SHORT_ID_LENGTH)}…` : id;
  switch (kind) {
    case 'session':
      return `Started by session ${shortId}`;
    case 'app':
      return `Started by a connected app (grant ${shortId})`;
    default:
      return `Started by ${kind} ${shortId}`;
  }
}
