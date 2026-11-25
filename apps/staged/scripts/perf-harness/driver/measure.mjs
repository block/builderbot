// One scenario's result, assembled from what the backend counted, what the
// page recorded and what the sampler saw. A figure that could not be taken
// is `null`, never zero.

import { distribution, histogramMs, processUsage, round } from './stats.mjs';

const commandTable = (commands) =>
  Object.fromEntries(
    Object.entries(commands).map(([name, c]) => [
      name,
      {
        count: c.count,
        errors: c.errors,
        wireBytes: c.wireBytes,
        meanMs: round(c.totalUs / c.count / 1000, 2),
        maxMs: round(c.maxUs / 1000, 2),
      },
    ])
  );
const total = (table, field) =>
  Object.values(table).reduce((sum, entry) => sum + (entry[field] ?? 0), 0);
const latency = (bySurface) =>
  Object.fromEntries(
    Object.entries(bySurface).map(([surface, kinds]) => [
      surface,
      { text: distribution(kinds.x), tool: distribution(kinds.t) },
    ])
  );

export function backendResult({ metrics, usage, frontendKind, streamSupported }) {
  // Desktop commands go over Tauri IPC, which the harness port never sees.
  const commands = frontendKind === 'browser' ? commandTable(metrics.commands) : null;
  return {
    process: processUsage(usage.app),
    childProcesses: processUsage(usage.children),
    commands,
    commandTotals: commands && {
      count: total(commands, 'count'),
      wireBytes: total(commands, 'wireBytes'),
    },
    streamFrames: streamSupported ? metrics.streamFrames : null,
    // `frames` and `bytes` are what a client's sink took. `dropped` is what
    // the hub sent and no sink took (a full queue, a client gone); hooks
    // before version 3 counted those as sent and have no such figure.
    streamTotals: streamSupported
      ? {
          frames: total(metrics.streamFrames, 'frames'),
          bytes: total(metrics.streamFrames, 'bytes'),
          dropped: (metrics.hooks ?? 1) >= 3 ? total(metrics.streamFrames, 'dropped') : null,
        }
      : null,
    storeLock: {
      waitMs: histogramMs(metrics.storeLock.wait),
      holdMs: histogramMs(metrics.storeLock.hold),
    },
    agentWriteToStoredMs: {
      text: histogramMs(metrics.stampToStored.text),
      tool: histogramMs(metrics.stampToStored.tool),
    },
  };
}

export function frontendResult({ collected, usage, seconds, streamSupported }) {
  const { probe, network, renderer } = collected;
  // The first gap after a reset spans the reset itself.
  const gaps = probe.gaps.slice(1);
  const commands = network ? network.commands : probe.tauri ? probe.ipc : null;
  // The event socket is not compressed unless the handshake said so; then a
  // frame's size on the wire is its size, and otherwise it is not known.
  const socketWire = (bytes) => (network.socketCompressed ? null : bytes);
  const eventBytes = network ? total(network.events, 'bytes') : null;
  const commandTotals = commands && {
    count: total(commands, 'count'),
    bytes: total(commands, 'bytes'),
    // Tauri IPC has no transport encoding: what is decoded is what was sent.
    wireBytes: network ? total(commands, 'wireBytes') : total(commands, 'bytes'),
  };
  return {
    commands,
    commandTotals,
    events: network ? network.events : null,
    streamFrames: network && streamSupported ? network.stream : null,
    streamTotals:
      network && streamSupported
        ? {
            frames: total(network.stream, 'frames'),
            bytes: total(network.stream, 'bytes'),
            wireBytes: socketWire(total(network.stream, 'bytes')),
          }
        : null,
    // Everything the page was sent, counted alike whichever way it came:
    // command responses plus every frame on the event socket (session
    // stream frames among them), once as the page reads it and once as it
    // crossed the transport. Browser only: the desktop's channels are not seen.
    received: network && {
      bytes: commandTotals.bytes + eventBytes,
      wireBytes: network.socketCompressed ? null : commandTotals.wireBytes + eventBytes,
      socketCompressed: network.socketCompressed,
    },
    longTasks: probe.longTaskSupported
      ? {
          count: probe.longTasks.length,
          totalMs: round(probe.longTasks.reduce((a, b) => a + b, 0)),
          maxMs: round(Math.max(0, ...probe.longTasks)),
        }
      : null,
    frameGapsMs: gaps.length
      ? {
          ...distribution(gaps),
          over50ms: gaps.filter((gap) => gap > 50).length,
          framesPerSecond: round(gaps.length / seconds),
        }
      : null,
    timerLagMs: distribution(probe.timerLag),
    agentWriteToDomMs: latency(probe.latency),
    agentWriteToFrameMs: latency(probe.frameLatency),
    domMutations: probe.mutations,
    renderer:
      renderer &&
      Object.fromEntries(Object.entries(renderer).map(([key, value]) => [key, round(value, 2)])),
    browserProcess:
      usage.browser.length && usage.browser[0].processes ? processUsage(usage.browser) : null,
    pageHidden: probe.hidden,
  };
}

/**
 * Which of the viewed project's rows were on the page, from `ui.rows` at
 * both ends of the window. A row counts as mounted only if it was there at
 * both: a card mounts its timeline near the visible part of the scroll area
 * and drops it again when scrolled away.
 */
export function mountedRows(atStart, atEnd) {
  const there = (rows) => new Set(rows.filter((row) => row.row).map((row) => row.tag));
  const [start, end] = [there(atStart), there(atEnd)];
  const both = atStart.map((row) => row.tag).filter((tag) => start.has(tag) && end.has(tag));
  return {
    mounted: both.length,
    mountedAtStart: start.size,
    mountedAtEnd: end.size,
    notMounted: atStart.map((row) => row.tag).filter((tag) => !both.includes(tag)),
  };
}

/**
 * What the fake agents wrote during the window, from their counter files
 * (written once a second). `rateShare` is that against `expectedPerSecond`,
 * what they write when nothing holds them back.
 */
export function agentLoad(before, after, seconds, expectedPerSecond = null) {
  const sum = (field) =>
    Object.keys(after).reduce(
      (total, tag) => total + (after[tag][field] - (before[tag]?.[field] ?? 0)),
      0
    );
  const chunks = sum('chunks');
  return {
    sessions: Object.keys(after).length,
    chunks,
    chunksPerSecond: round(chunks / seconds),
    expectedChunksPerSecond: expectedPerSecond === null ? null : round(expectedPerSecond),
    rateShare: expectedPerSecond > 0 ? round(chunks / seconds / expectedPerSecond, 3) : null,
    toolCalls: sum('toolCalls'),
    plans: sum('plans'),
    bytes: sum('bytes'),
  };
}
