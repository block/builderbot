import type { PrFailedCheck } from '../../types';
import { formatPreciseRelativeTime } from '../../shared/relativeTime';

export interface PrButtonTooltipInput {
  actionTitle: string;
  prNumber: number | null;
  prHeadSha: string | null;
  prFetchedAt: number | null;
  checksStatus: string | null;
  statusStale: boolean;
  statusRefreshing: boolean;
  hasUnpushed: boolean;
  failedChecks: PrFailedCheck[];
  statusCleared: boolean;
  nowMs?: number;
}

function formatSha(sha: string | null): string {
  return sha ? sha.slice(0, 7) : 'unknown';
}

function formatChecksStatus(status: string | null): string {
  switch (status) {
    case 'SUCCESS':
      return 'succeeded';
    case 'FAILURE':
      return 'failed';
    case 'PENDING':
      return 'pending';
    case 'EXPECTED':
    case null:
      return 'unknown';
    default:
      return status.toLowerCase();
  }
}

function formatFailedChecks(failedChecks: PrFailedCheck[]): string {
  return failedChecks.map((check) => `${check.name} (${check.state})`).join(', ');
}

export function buildPrButtonTitle(input: PrButtonTooltipInput): string {
  const lines = [input.actionTitle];

  if (!input.prNumber) return lines.join('\n');

  lines.push(`Latest PR SHA: ${formatSha(input.prHeadSha)}`);

  if (input.hasUnpushed) {
    lines.push('Local branch has newer commits than the PR');
    if (input.prFetchedAt) {
      lines.push(`Last checked: ${formatPreciseRelativeTime(input.prFetchedAt, input.nowMs)}`);
    } else {
      lines.push('Last checked: unknown');
    }
    if (input.statusRefreshing) {
      lines.push('Status refresh in progress');
    }
    return lines.join('\n');
  }

  if (input.prFetchedAt) {
    lines.push(`Last checked: ${formatPreciseRelativeTime(input.prFetchedAt, input.nowMs)}`);
  } else {
    lines.push('Last checked: unknown');
  }

  if (input.statusRefreshing) {
    lines.push('Status refresh in progress');
  }

  if (input.statusCleared) {
    lines.push('Previous PR status was cleared; waiting for next GitHub refresh');
    return lines.join('\n');
  }

  lines.push(`Checks: ${formatChecksStatus(input.checksStatus)}`);

  if (input.failedChecks.length > 0) {
    lines.push(`Failed: ${formatFailedChecks(input.failedChecks)}`);
  }

  if (input.statusStale) {
    lines.push('Status refresh may be outdated');
  }

  return lines.join('\n');
}
