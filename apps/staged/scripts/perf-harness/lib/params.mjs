// The harness's parameters: every knob of the load and of the run, with its
// default. `run.mjs --help` prints this table.

export const PARAMS = {
  // Load shape.
  projects: [
    3,
    'projects with sessions running at once: the one the page views and the rest in the background',
  ],
  'sessions-per-project': [4, 'sessions (one branch each) per project'],
  running: [0, 'how many sessions stream at once (0 = all of them)'],
  rate: [4, 'text chunks per second from each background session'],
  'chat-rate': [20, 'text chunks per second from the session whose chat is open'],
  'chunk-chars': [80, 'characters of text per chunk, after its stamp'],
  'paragraph-every': [8, 'start a new paragraph every N chunks (0 = never)'],
  'tool-every': [25, 'make a tool call every N chunks (0 = never)'],
  'tool-ms': [2000, 'how long a tool call stays in flight, streaming its output'],
  'tool-step-ms': [500, 'how often an in-flight tool call’s output grows'],
  'tool-bytes': [6000, 'size of a tool call body'],
  'large-tool-every': [4, 'every Nth tool call has a large body (0 = never)'],
  'large-tool-bytes': [200000, 'size of a large tool call body'],
  'plan-every': [40, 'send a plan and a title update every N chunks (0 = never)'],
  'prompt-bytes': [
    0,
    'pad each session’s opening prompt to this many bytes (0 = two words; the app wraps either in its own text)',
  ],
  // The run.
  scenarios: ['subtitles,chat,both,hidden', 'which scenarios to run, in this order'],
  duration: [30, 'seconds measured per scenario'],
  warmup: [5, 'seconds between a scenario reaching its view and the measurement starting'],
  'viewport-height': [
    1000,
    'height of the browser window in px; a project’s rows are mounted only near what it shows',
  ],
  'start-timeout': [240, 'seconds to wait for every session of a scenario to be streaming'],
  'min-rate-share': [
    0.5,
    'stop the run when the agents write less than this share of the rate asked for, or the process sampler gets less than this share of its samples (0 = never)',
  ],
  'skip-verify': [false, 'skip the checks that what the page shows is what the store holds'],
  frontend: [
    'browser',
    'browser (headless Chromium over the loopback server) or desktop (the app’s own webview)',
  ],
  'local-storage': [
    [],
    'key=value set in the page’s localStorage before the app boots; repeatable',
  ],
  label: ['run', 'name for the result files'],
  out: ['', 'directory for the result files (default: <tmp>/staged-perf-harness/results)'],
  // The build.
  'app-dir': ['', 'apps/staged of the tree to build and measure (default: this one)'],
  profile: ['debug', 'cargo profile: debug or release'],
  port: [
    47611,
    'loopback port; it is compiled into the app as its dev URL, so changing it rebuilds',
  ],
  'skip-build': [false, 'reuse the binary and frontend built by an earlier run of this tree'],
  'target-dir': [
    '',
    'cargo target directory for the build (default: one of the harness’s own, never the tree’s)',
  ],
  browser: [
    '',
    'Chromium executable (default: $BROWSER_PATH, then a Playwright cache or Chrome install)',
  ],
  keep: [false, 'keep the throwaway run directory instead of deleting it'],
};

export const SCENARIOS = ['subtitles', 'chat', 'both', 'hidden'];

const camel = (name) => name.replace(/-(\w)/g, (_, c) => c.toUpperCase());

/** `--name value` pairs (and bare `--flag` for booleans) into an object keyed in camelCase. */
export function parseParams(argv) {
  const out = Object.fromEntries(
    Object.entries(PARAMS).map(([name, [value]]) => [
      camel(name),
      Array.isArray(value) ? [] : value,
    ])
  );
  for (let i = 0; i < argv.length; i++) {
    const name = argv[i].startsWith('--') ? argv[i].slice(2) : null;
    if (!name || !(name in PARAMS)) throw new Error(`unknown argument: ${argv[i]}`);
    const [fallback] = PARAMS[name];
    const key = camel(name);
    if (typeof fallback === 'boolean') {
      out[key] = true;
      continue;
    }
    const value = argv[++i];
    if (value === undefined) throw new Error(`--${name} needs a value`);
    if (Array.isArray(fallback)) out[key].push(value);
    else if (typeof fallback === 'number') {
      out[key] = Number(value);
      if (!Number.isFinite(out[key]) || out[key] < 0)
        throw new Error(`--${name} must be a number, not ${value}`);
    } else out[key] = value;
  }
  out.scenarios = String(out.scenarios).split(',').filter(Boolean);
  for (const scenario of out.scenarios) {
    if (!SCENARIOS.includes(scenario))
      throw new Error(`unknown scenario: ${scenario} (known: ${SCENARIOS.join(', ')})`);
  }
  if (!['browser', 'desktop'].includes(out.frontend))
    throw new Error(`--frontend must be browser or desktop`);
  if (!['debug', 'release'].includes(out.profile))
    throw new Error(`--profile must be debug or release`);
  if (out.projects < 2)
    throw new Error('--projects must be at least 2: the load has to be spread across projects');
  if (out.sessionsPerProject < 1) throw new Error('--sessions-per-project must be at least 1');
  if (out.toolStepMs < 1) throw new Error('--tool-step-ms must be at least 1');
  if (out.viewportHeight < 200) throw new Error('--viewport-height must be at least 200');
  if (out.minRateShare > 1) throw new Error('--min-rate-share is a share, between 0 and 1');
  if (out.frontend === 'desktop' && out.viewportHeight !== PARAMS['viewport-height'][0])
    throw new Error(
      '--viewport-height sizes the browser window; the desktop webview keeps the size the app gives it'
    );
  out.localStorage = Object.fromEntries(
    out.localStorage.map((pair) => {
      const at = pair.indexOf('=');
      if (at < 1) throw new Error(`--local-storage takes key=value, not ${pair}`);
      return [pair.slice(0, at), pair.slice(at + 1)];
    })
  );
  return out;
}

export function usage() {
  const rows = Object.entries(PARAMS).map(([name, [value, help]]) => {
    const shown = Array.isArray(value)
      ? ''
      : typeof value === 'boolean'
        ? ''
        : ` (default ${value === '' ? 'see text' : value})`;
    return `  --${name.padEnd(22)} ${help}${shown}`;
  });
  return [
    'usage: run.mjs [options]',
    '       run.mjs compare <a.json> <b.json> [--all]',
    '',
    ...rows,
  ].join('\n');
}
