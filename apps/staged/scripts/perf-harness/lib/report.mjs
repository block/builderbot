// A result file as text: the short summary a run writes beside its JSON, and
// the difference between two result files.

/** The figures worth reading first, as paths inside one scenario. */
export const HEADLINES = [
  ['load.sessions', 'sessions streaming'],
  ['load.chunksPerSecond', 'agent chunks / s'],
  ['backend.process.cpuPercent', 'app CPU %'],
  ['backend.process.cpuSeconds', 'app CPU s'],
  ['backend.process.rssMaxMb', 'app RSS max MB'],
  ['backend.childProcesses.cpuSeconds', 'app children CPU s'],
  ['backend.commandTotals.count', 'commands served'],
  ['backend.commandTotals.wireBytes', 'command bytes sent (wire)'],
  ['backend.streamTotals.frames', 'stream frames sent'],
  ['backend.streamTotals.bytes', 'stream bytes sent (JSON)'],
  ['backend.storeLock.waitMs.count', 'store lock takes'],
  ['backend.storeLock.waitMs.p95', 'store lock wait p95 ms'],
  ['backend.storeLock.waitMs.p99', 'store lock wait p99 ms'],
  ['backend.storeLock.waitMs.max', 'store lock wait max ms'],
  ['backend.storeLock.holdMs.p95', 'store lock hold p95 ms'],
  ['backend.storeLock.holdMs.max', 'store lock hold max ms'],
  ['backend.agentWriteToStoredMs.text.p50', 'agent text → stored p50 ms'],
  ['backend.agentWriteToStoredMs.text.p95', 'agent text → stored p95 ms'],
  ['backend.agentWriteToStoredMs.tool.p50', 'agent tool call → stored p50 ms'],
  ['backend.agentWriteToStoredMs.tool.p95', 'agent tool call → stored p95 ms'],
  ['frontend.commandTotals.count', 'page calls'],
  ['frontend.commandTotals.bytes', 'page call bytes (decoded)'],
  ['frontend.streamTotals.frames', 'page stream frames'],
  ['frontend.streamTotals.bytes', 'page stream bytes'],
  ['frontend.longTasks.count', 'long tasks'],
  ['frontend.longTasks.totalMs', 'long task ms'],
  ['frontend.frameGapsMs.p50', 'frame gap p50 ms'],
  ['frontend.frameGapsMs.p95', 'frame gap p95 ms'],
  ['frontend.frameGapsMs.p99', 'frame gap p99 ms'],
  ['frontend.frameGapsMs.max', 'frame gap max ms'],
  ['frontend.timerLagMs.p95', 'timer lag p95 ms'],
  ['frontend.agentWriteToDomMs.subtitle.text.p50', 'agent text → subtitle p50 ms'],
  ['frontend.agentWriteToDomMs.subtitle.text.p95', 'agent text → subtitle p95 ms'],
  ['frontend.agentWriteToDomMs.subtitle.tool.p50', 'agent tool call → subtitle p50 ms'],
  ['frontend.agentWriteToDomMs.chat.text.p50', 'agent text → chat p50 ms'],
  ['frontend.agentWriteToDomMs.chat.text.p95', 'agent text → chat p95 ms'],
  ['frontend.agentWriteToDomMs.chat.text.count', 'chat text chunks seen'],
  ['frontend.renderer.taskSeconds', 'renderer task s'],
  ['frontend.renderer.scriptSeconds', 'renderer script s'],
  ['frontend.browserProcess.cpuSeconds', 'browser CPU s'],
  ['frontend.catchUpAfterShownMs', 'catch-up after shown ms'],
];

const at = (object, path) =>
  path
    .split('.')
    .reduce(
      (value, key) => (value === null || value === undefined ? undefined : value[key]),
      object
    );
const show = (value) =>
  value === null || value === undefined
    ? 'absent'
    : typeof value === 'number'
      ? String(Number(value.toFixed(2)))
      : String(value);
const table = (header, rows) => {
  const widths = header.map((_, i) =>
    Math.max(...[header, ...rows].map((row) => String(row[i]).length))
  );
  const line = (row) =>
    `| ${row.map((cell, i) => (i === 0 ? String(cell).padEnd(widths[i]) : String(cell).padStart(widths[i]))).join(' | ')} |`;
  return [
    line(header),
    `| ${widths.map((w, i) => (i === 0 ? '-'.repeat(w) : '-'.repeat(w - 1) + ':')).join(' | ')} |`,
    ...rows.map(line),
  ].join('\n');
};

/** Every numeric leaf of an object as `[path, value]`. */
export function numericLeaves(object, prefix = '') {
  const out = [];
  for (const [key, value] of Object.entries(object ?? {})) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'number') out.push([path, value]);
    else if (value && typeof value === 'object' && !Array.isArray(value))
      out.push(...numericLeaves(value, path));
  }
  return out;
}

export function summarize(result) {
  const names = Object.keys(result.scenarios);
  const rows = HEADLINES.map(([path, label]) => [
    label,
    ...names.map((name) => show(at(result.scenarios[name], path))),
  ]).filter((row) => row.slice(1).some((cell) => cell !== 'absent'));
  const safety = result.safety;
  return [
    `# Staged load test: ${result.label}`,
    '',
    `- tree: ${result.tree.head?.slice(0, 12) ?? 'unknown'} (${result.tree.branch ?? '?'}${result.tree.dirty ? ', with uncommitted changes' : ''}), ${result.params.profile} build`,
    `- frontend measured: ${result.frontend === 'browser' ? 'headless Chromium over the loopback web server' : 'the desktop webview over Tauri IPC'}`,
    `- session stream: ${result.streamSupported ? 'present in this tree' : 'not in this tree (stream figures absent)'}`,
    `- localStorage overrides: ${Object.keys(result.params.localStorage).length ? JSON.stringify(result.params.localStorage) : 'none'}`,
    `- load: ${result.params.projects} projects × ${result.params.sessionsPerProject} sessions, ${result.params.running || 'all'} running, ${result.params.rate} chunks/s background, ${result.params.chatRate} chunks/s chat, ${result.params.duration}s per scenario`,
    `- machine load average: ${result.host.loadAverageStart.map((n) => n.toFixed(1)).join(' ')} at the start, ${result.host.loadAverageEnd.map((n) => n.toFixed(1)).join(' ')} at the end`,
    '',
    table(['', ...names], rows),
    '',
    '## Safety',
    '',
    `- data directory: ${safety.dataDir} (inside the run directory ${safety.runDir})`,
    `- files open under the real ${safety.realStagedDir}: ${safety.openUnderRealStaged.length} in ${safety.openFileSamples} samples of the run's processes${safety.openUnderRealStaged.length ? `: ${safety.openUnderRealStaged.join('; ')}` : ''}`,
    `- programs from outside the run directory and the system: ${safety.unexpectedExecutables.length ? safety.unexpectedExecutables.join('; ') : 'none'}`,
    `- processes left after the run: ${safety.leftoverProcesses.length ? safety.leftoverProcesses.map((p) => `${p.pid} ${p.args}`).join('; ') : 'none'}`,
    '',
  ].join('\n');
}

/** The difference between two result files, scenario by scenario. `all` lists every numeric figure. */
export function compare(a, b, { all = false } = {}) {
  const out = [
    `A: ${a.label} (${a.tree.head?.slice(0, 12) ?? '?'}, ${a.frontend}, localStorage ${JSON.stringify(a.params.localStorage)})`,
    `B: ${b.label} (${b.tree.head?.slice(0, 12) ?? '?'}, ${b.frontend}, localStorage ${JSON.stringify(b.params.localStorage)})`,
  ];
  const differing = Object.keys(a.params).filter(
    (key) =>
      !['label', 'out', 'localStorage', 'keep', 'skipBuild'].includes(key) &&
      JSON.stringify(a.params[key]) !== JSON.stringify(b.params[key])
  );
  if (a.frontend !== b.frontend) out.push('warning: the two runs measured different frontends');
  if (differing.length)
    out.push(
      `warning: the two runs differ in ${differing.join(', ')}; their figures are not like for like`
    );
  for (const name of new Set([...Object.keys(a.scenarios), ...Object.keys(b.scenarios)])) {
    const [left, right] = [a.scenarios[name], b.scenarios[name]];
    out.push('', `## ${name}`, '');
    if (!left || !right) {
      out.push(`only in ${left ? 'A' : 'B'}`);
      continue;
    }
    const paths = all
      ? [...new Set([...numericLeaves(left), ...numericLeaves(right)].map(([path]) => path))].map(
          (path) => [path, path]
        )
      : HEADLINES;
    const rows = [];
    for (const [path, label] of paths) {
      const [x, y] = [at(left, path), at(right, path)];
      if ((x === null || x === undefined) && (y === null || y === undefined)) continue;
      const both = typeof x === 'number' && typeof y === 'number';
      const delta = both ? y - x : null;
      rows.push([
        label,
        show(x),
        show(y),
        both ? `${delta >= 0 ? '+' : ''}${show(delta)}` : '',
        both && x !== 0 && show(x) !== show(y)
          ? `${delta >= 0 ? '+' : ''}${((delta / x) * 100).toFixed(0)}%`
          : '',
      ]);
    }
    out.push(table(['', 'A', 'B', 'B − A', ''], rows));
  }
  return out.join('\n');
}
