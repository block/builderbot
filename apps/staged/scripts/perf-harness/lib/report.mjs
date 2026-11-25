// A result file as text: the short summary a run writes beside its JSON, and
// the difference between two result files.

/** The figures worth reading first, as paths inside one scenario. */
export const HEADLINES = [
  ['host.loadAverage1mStart', 'machine load average at the start'],
  ['load.sessions', 'sessions streaming'],
  ['load.chunksPerSecond', 'agent chunks / s'],
  ['load.rateShare', 'share of the rate asked for'],
  ['load.viewedSessions', 'sessions running in the viewed project'],
  ['load.rows.mounted', 'their rows mounted on the page'],
  ['backend.process.cpuPercent', 'app CPU %'],
  ['backend.process.cpuSeconds', 'app CPU s'],
  ['backend.process.rssMaxMb', 'app RSS max MB'],
  ['backend.childProcesses.cpuSeconds', 'app children CPU s'],
  ['backend.commandTotals.count', 'commands served'],
  ['backend.commandTotals.wireBytes', 'command bytes sent (wire)'],
  ['backend.streamTotals.frames', 'stream frames sent'],
  ['backend.streamTotals.dropped', 'stream frames no client took (queue full, client gone)'],
  ['backend.streamTotals.bytes', 'stream bytes sent (JSON, before the transport)'],
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
  ['frontend.commandTotals.wireBytes', 'page call bytes (wire)'],
  ['frontend.streamTotals.frames', 'page stream frames'],
  ['frontend.streamTotals.bytes', 'page stream bytes (decoded)'],
  ['frontend.streamTotals.wireBytes', 'page stream bytes (wire)'],
  ['frontend.received.bytes', 'page bytes received, calls and socket (decoded)'],
  ['frontend.received.wireBytes', 'page bytes received, calls and socket (wire)'],
  ['frontend.longTasks.count', 'long tasks'],
  ['frontend.longTasks.totalMs', 'long task ms'],
  ['frontend.frameGapsMs.p50', 'frame gap p50 ms'],
  ['frontend.frameGapsMs.p95', 'frame gap p95 ms'],
  ['frontend.frameGapsMs.p99', 'frame gap p99 ms'],
  ['frontend.frameGapsMs.max', 'frame gap max ms'],
  ['frontend.timerLagMs.p95', 'timer lag p95 ms'],
  ['frontend.subtitleStaleness.outdatedMs.mean', 'subtitle out of date, mean ms'],
  ['frontend.subtitleStaleness.outdatedMs.p50', 'subtitle out of date p50 ms'],
  ['frontend.subtitleStaleness.outdatedMs.p95', 'subtitle out of date p95 ms'],
  ['frontend.subtitleStaleness.outdatedMs.max', 'subtitle out of date max ms'],
  ['frontend.subtitleStaleness.outdatedMs.count', 'subtitle staleness samples'],
  ['frontend.agentWriteToDomMs.subtitle.text.count', 'subtitle text updates shown'],
  ['frontend.agentWriteToDomMs.subtitle.text.p50', 'agent text → subtitle p50 ms (of those shown)'],
  ['frontend.agentWriteToDomMs.subtitle.tool.p50', 'agent tool call → subtitle p50 ms'],
  ['frontend.agentWriteToDomMs.subtitle.tool.p95', 'agent tool call → subtitle p95 ms'],
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
    // A column with nothing in it (two runs that do not differ) is still a column.
    `| ${widths.map((w, i) => (i === 0 ? '-'.repeat(Math.max(1, w)) : '-'.repeat(Math.max(1, w - 1)) + ':')).join(' | ')} |`,
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

/** One line for a check: what was compared and what did not match. */
function checkLine(name, check, outcome) {
  const verdict = outcome.ok ? 'ok' : 'FAILED';
  if (check === 'subtitles') {
    const problems = [
      outcome.late.length &&
        `${outcome.lateCount ?? outcome.late.length} rows never showed what the store held (${outcome.late[0]})`,
      outcome.unknown.length &&
        `${outcome.unknownCount ?? outcome.unknown.length} stamps shown that the store does not hold (${outcome.unknown[0]})`,
      outcome.rows === 0 && 'no row to check',
    ].filter(Boolean);
    return `- ${name}, subtitles: ${verdict}; ${outcome.rows} row${outcome.rows === 1 ? '' : 's'} over ${outcome.rounds} rounds, each showing what the store held within ${show(outcome.caughtUpMsMax)} ms${problems.length ? `; ${problems.join('; ')}` : ''}`;
  }
  const problems = [
    outcome.unknown.length &&
      `${outcome.unknownCount ?? outcome.unknown.length} shown that the store does not hold (first seq ${outcome.unknown[0]})`,
    outcome.missing.length &&
      `${outcome.missingCount ?? outcome.missing.length} stored and not shown (first seq ${outcome.missing[0]})`,
    outcome.shown === 0 && 'no text stamp in the chat',
  ].filter(Boolean);
  return `- ${name}, chat: ${verdict}; ${outcome.shown} text stamps shown, ${outcome.stored} stored, newest ${show(outcome.newestShown)} against ${show(outcome.newestStored)}${problems.length ? `; ${problems.join('; ')}` : ''}`;
}

/**
 * The agents of a run: which were stubbed, how often a stub was started,
 * and what the app's log says it started. Nothing for a result that
 * predates the stubs.
 */
function agentLines(agents) {
  if (!agents) return [];
  const starts = Object.entries(agents.stubStarts);
  const total = starts.reduce((sum, [, times]) => sum + times, 0);
  return [
    `- agents stubbed in the run's tools directory: ${agents.stubbed.join(', ')}; stub starts: ${total}${total ? ` (${starts.map(([name, times]) => `${name} ${times}`).join(', ')})` : ''}`,
    agents.startsLogged === null
      ? `- agent starts in the app's log: not read (this build does not log them)`
      : `- agent starts in the app's log: ${agents.startsLogged}, ${agents.startsLoggedOutside} of them outside the run directory`,
  ];
}

export function summarize(result) {
  // A scenario that failed has no figures; it gets a line, not a column.
  const names = Object.keys(result.scenarios).filter((name) => !result.scenarios[name].failure);
  const rows = HEADLINES.map(([path, label]) => [
    label,
    ...names.map((name) => show(at(result.scenarios[name], path))),
  ]).filter((row) => row.slice(1).some((cell) => cell !== 'absent'));
  const safety = result.safety;
  const failed = Object.entries(result.scenarios).filter(([, scenario]) => scenario.failure);
  const notRun = result.notRun ?? [];
  const status = result.failure
    ? [
        `**FAILED while ${result.failure.phase}: ${result.failure.message}**`,
        '',
        names.length
          ? `The figures below are from the ${names.length === 1 ? 'scenario' : `${names.length} scenarios`} measured before that (${names.join(', ')}).${notRun.length ? ` Not run: ${notRun.join(', ')}.` : ''}`
          : `Nothing was measured.${notRun.length ? ` Not run: ${notRun.join(', ')}.` : ''}`,
        '',
      ]
    : result.complete === false
      ? ['**INCOMPLETE: the run was stopped before it finished.**', '']
      : [];
  const checks = names.flatMap((name) =>
    Object.entries(result.scenarios[name].checks ?? {}).map(([check, outcome]) =>
      checkLine(name, check, outcome)
    )
  );
  const loadAverage = (values) => (values ? values.map((n) => n.toFixed(1)).join(' ') : 'unknown');
  return [
    `# Staged load test: ${result.label}`,
    '',
    ...status,
    `- tree: ${result.tree.head?.slice(0, 12) ?? 'unknown'} (${result.tree.branch ?? '?'}${result.tree.dirty ? ', with uncommitted changes' : ''}), ${result.params.profile} build`,
    `- frontend measured: ${result.frontend === 'browser' ? `headless Chromium over the loopback web server, ${result.params.viewportHeight ?? 1000} px tall` : 'the desktop webview over Tauri IPC'}`,
    `- session stream: ${result.streamSupported === null ? 'not asked' : result.streamSupported ? 'present in this tree' : 'not in this tree (stream figures absent)'}`,
    `- localStorage overrides: ${Object.keys(result.params.localStorage).length ? JSON.stringify(result.params.localStorage) : 'none'}`,
    `- load: ${result.params.projects} projects × ${result.params.sessionsPerProject} sessions, ${result.params.running || 'all'} running, ${result.params.rate} chunks/s background, ${result.params.chatRate} chunks/s chat, ${result.params.duration}s per scenario`,
    `- machine load average: ${loadAverage(result.host.loadAverageStart)} at the start, ${loadAverage(result.host.loadAverageEnd)} at the end`,
    '',
    ...(names.length ? [table(['', ...names], rows), ''] : []),
    ...(failed.length
      ? [
          '## Failed',
          '',
          ...failed.map(
            ([name, scenario]) =>
              `- ${name}, while ${scenario.failure.phase}: ${scenario.failure.message}`
          ),
          '',
        ]
      : []),
    ...(checks.length ? ['## Shown against stored', '', ...checks, ''] : []),
    ...(result.warnings?.length
      ? ['## Read before trusting the figures', '', ...result.warnings.map((w) => `- ${w}`), '']
      : []),
    '## Safety',
    '',
    ...(safety
      ? [
          `- data directory: ${safety.dataDir ?? 'the app never reported one'} (inside the run directory ${safety.runDir})`,
          `- files open under the real ${safety.realStagedDir}: ${safety.openUnderRealStaged.length} in ${safety.openFileSamples} samples of the run's processes${safety.openUnderRealStaged.length ? `: ${safety.openUnderRealStaged.join('; ')}` : ''}`,
          `- programs from outside the run directory and the system: ${safety.unexpectedExecutables.length ? safety.unexpectedExecutables.join('; ') : 'none'}`,
          ...agentLines(safety.agents),
          `- processes left after the run: ${safety.leftoverProcesses ? (safety.leftoverProcesses.length ? safety.leftoverProcesses.map((p) => `${p.pid} ${p.args}`).join('; ') : 'none') : 'not checked'}`,
        ]
      : ['- the run stopped before anything was checked']),
    '',
  ].join('\n');
}

/**
 * Parameters that say how a run was named, built or served, and nothing
 * about what was measured: two runs that differ in these alone are like for
 * like. `appDir` and `targetDir` are here because the documented way to
 * compare two trees gives the second run an `--app-dir`.
 */
const NOT_THE_LOAD = [
  'label',
  'out',
  'localStorage',
  'keep',
  'skipBuild',
  'appDir',
  'targetDir',
  'buildWait',
  'port',
];

/** The parameters whose values differ between two runs and make their figures not like for like. */
export function differingParams(a, b) {
  return [...new Set([...Object.keys(a), ...Object.keys(b)])].filter(
    (key) => !NOT_THE_LOAD.includes(key) && JSON.stringify(a[key]) !== JSON.stringify(b[key])
  );
}

/** The difference between two result files, scenario by scenario. `all` lists every numeric figure. */
export function compare(a, b, { all = false } = {}) {
  const out = [
    `A: ${a.label} (${a.tree.head?.slice(0, 12) ?? '?'}, ${a.frontend}, localStorage ${JSON.stringify(a.params.localStorage)})`,
    `B: ${b.label} (${b.tree.head?.slice(0, 12) ?? '?'}, ${b.frontend}, localStorage ${JSON.stringify(b.params.localStorage)})`,
  ];
  const differing = differingParams(a.params, b.params);
  if (a.frontend !== b.frontend) out.push('warning: the two runs measured different frontends');
  for (const [side, result] of [
    ['A', a],
    ['B', b],
  ]) {
    if (result.failure) out.push(`warning: ${side} failed: ${result.failure.message}`);
    if (result.checks && !result.checks.ok)
      out.push(
        `warning: ${side} showed something its store did not hold: ${result.checks.failed.join(', ')}`
      );
  }
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
    if (left.failure || right.failure) {
      for (const [side, scenario] of [
        ['A', left],
        ['B', right],
      ])
        if (scenario.failure) out.push(`failed in ${side}: ${scenario.failure.message}`);
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
