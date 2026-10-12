/**
 * Copy for the "Detect existing PR" outcome dialog.
 *
 * The backend reports structured reasons so the matching rules live in one
 * place; this turns them into sentences the rejected-PR dialog can list.
 */

import type { PrRejectionReason } from '../../types';

export function describePrRejectionReason(reason: PrRejectionReason): string {
  switch (reason.kind) {
    case 'notOpen':
      return `It is ${reason.state}; only an open pull request is linked.`;
    case 'wrongBase':
      return `It targets ${reason.actual}, but this branch is based on ${reason.expected}.`;
    case 'wrongHeadRepo':
      return reason.actual
        ? `Its head branch lives in ${reason.actual}, not ${reason.expected}.`
        : `Its head repository no longer exists (this branch pushes to ${reason.expected}).`;
  }
}
