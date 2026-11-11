/**
 * Listener for `stagedctl connect` consent requests.
 *
 * The control server emits `control-authorization-requested` (bare
 * `app.emit`, see `control_server/consent.rs`) for every request a CLI makes
 * outside a spawned session. Registration is asynchronous, so a request that
 * arrives before the listener is live would be lost; once the listener is
 * established the backend's snapshot of still-pending requests is taken as
 * well, and the controller dedupes the overlap.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import { controlAuthorization } from '../features/settings/controlAuthorization.svelte';
import type { ControlAuthorizationRequest } from '../types';

/** Mirrors `AUTHORIZATION_REQUESTED_EVENT` in `control_server/consent.rs`. */
export const CONTROL_AUTHORIZATION_REQUESTED_EVENT = 'control-authorization-requested';

export function listenForControlAuthorizations(): UnlistenFn {
  return listenToEvent<ControlAuthorizationRequest>(
    CONTROL_AUTHORIZATION_REQUESTED_EVENT,
    (request) => controlAuthorization.offer(request),
    { onEstablished: () => void controlAuthorization.loadPending() }
  );
}
