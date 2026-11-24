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

export function backendResult({ metrics, usage, seconds, frontendKind, streamSupported }) {
  // Desktop commands go over Tauri IPC, which the harness port never sees.
  const commands = frontendKind === 'browser' ? commandTable(metrics.commands) : null;
  return {
    process: processUsage(usage.app, seconds),
    childProcesses: processUsage(usage.children, seconds),
    commands,
    commandTotals: commands && {
      count: total(commands, 'count'),
      wireBytes: total(commands, 'wireBytes'),
    },
    streamFrames: streamSupported ? metrics.streamFrames : null,
    streamTotals: streamSupported
      ? {
          frames: total(metrics.streamFrames, 'frames'),
          bytes: total(metrics.streamFrames, 'bytes'),
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
  return {
    commands,
    commandTotals: commands && { count: total(commands, 'count'), bytes: total(commands, 'bytes') },
    events: network ? network.events : null,
    streamFrames: network && streamSupported ? network.stream : null,
    streamTotals:
      network && streamSupported
        ? { frames: total(network.stream, 'frames'), bytes: total(network.stream, 'bytes') }
        : null,
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
      usage.browser.length && usage.browser[0].processes
        ? processUsage(usage.browser, seconds)
        : null,
    pageHidden: probe.hidden,
  };
}

/** What the fake agents wrote during the window, from their counter files (written once a second). */
export function agentLoad(before, after, seconds) {
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
    toolCalls: sum('toolCalls'),
    plans: sum('plans'),
    bytes: sum('bytes'),
  };
}
