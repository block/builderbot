// A load run starts no agent of the user's: every agent the app knows is
// stubbed in the run directory, the login shell the app asks for a
// program's place answers from the run's PATH, and a program started from
// anywhere else fails the run. These tests hold each of the three.

import { spawnSync } from 'node:child_process';
import {
  appendFileSync,
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { outsideProgramsFailure, watchPrograms } from './driver/programs.mjs';
import {
  AGENT_SOURCES,
  FAKE_AGENT_COMMAND,
  agentCommands,
  commandsIn,
  listBody,
  stubStarts,
} from './lib/agents.mjs';
import { parseParams } from './lib/params.mjs';
import { summarize } from './lib/report.mjs';
import {
  expectedPlaces,
  loginFiles,
  outsidePlace,
  outsideProgram,
  runPath,
  scrubbedEnv,
} from './lib/safety.mjs';
import { SPAWN_LOG_FORMAT, followSpawns, spawnLogProblem, spawnsIn } from './lib/spawns.mjs';
import { stage } from './lib/stage.mjs';

const APP_DIR = join(import.meta.dirname, '..', '..');
const REPO = join(APP_DIR, '..', '..');

let scratch: string;
beforeEach(() => {
  scratch = mkdtempSync(join(realpathSync(tmpdir()), 'perf-harness-agents-'));
});
afterEach(() => {
  rmSync(scratch, { recursive: true, force: true });
});

const executable = (path: string, text = '#!/bin/sh\nexit 0\n') => {
  writeFileSync(path, text);
  chmodSync(path, 0o755);
};

/** A tree laid out as the repository is, with the two agent lists as given. */
function treeWith(known: string, checks: string) {
  const appDir = join(scratch, 'tree', 'apps', 'staged');
  mkdirSync(appDir, { recursive: true });
  for (const [file, text] of [
    ['crates/acp-client/src/types.rs', known],
    ['crates/doctor/src/agents.rs', checks],
  ]) {
    mkdirSync(join(scratch, 'tree', file, '..'), { recursive: true });
    writeFileSync(join(scratch, 'tree', file), text);
  }
  return appDir;
}
const KNOWN = (commands: string[]) =>
  `pub(crate) const KNOWN_AGENTS: &[KnownAgent] = &[\n${commands
    .map(
      (command) =>
        `    KnownAgent {\n        id: "x",\n        command: "${command}",\n        acp_args: &["acp"],\n    },\n`
    )
    .join('')}];\n`;
const CHECKS = `pub const AI_AGENT_CHECKS: &[AgentCheckInfo] = &[
    AgentCheckInfo {
        id: "ai-agent-pi",
        commands: &["pi-acp", "pi-old-acp"],
        main_command: Some("pi"),
        install_command: Some("curl -fsSL https://pi.dev/install.sh | sh"),
        auth_command: None,
        auth_status_command: Some("pi status"),
    },
];
`;

/** A run directory staged from this tree, with stand-ins for the build's output. */
function stagedRun(appDir = APP_DIR) {
  const runDir = join(scratch, 'run');
  const home = join(runDir, 'home');
  mkdirSync(home, { recursive: true });
  const binaryDir = join(scratch, 'built');
  mkdirSync(join(scratch, 'dist'), { recursive: true });
  mkdirSync(binaryDir, { recursive: true });
  writeFileSync(join(binaryDir, 'Staged'), '');
  const built = { binary: join(binaryDir, 'Staged'), binaryDir, dist: join(scratch, 'dist') };
  return { runDir, home, staged: stage({ runDir, home, appDir, built, profile: 'debug' }) };
}

describe('the agents the app knows', () => {
  it('are read from the tree: every entry of both lists, and nothing else', () => {
    const commands = agentCommands(APP_DIR);
    // The five a session or action detection can start, whatever else is added.
    for (const name of ['goose', 'claude-agent-acp', 'codex-acp', 'pi-acp', 'amp-acp'])
      expect(commands).toContain(name);
    // Counted another way: each entry of a list is one struct literal with
    // one field naming its commands. A list the reader only half
    // understands, or a second list of the same struct in the file, has
    // more literals than the reader found fields.
    for (const [{ file, list, fields }, struct] of [
      [AGENT_SOURCES[0], 'KnownAgent'],
      [AGENT_SOURCES[1], 'AgentCheckInfo'],
    ] as const) {
      const source = readFileSync(join(REPO, file), 'utf8');
      const literals = source.match(new RegExp(`^\\s+${struct} \\{$`, 'gm')) ?? [];
      const body = listBody(source, list);
      expect(body, `${list} in ${file}`).not.toBeNull();
      const named = [...body!.matchAll(new RegExp(`(?<![A-Za-z0-9_])${fields[0]}\\s*:`, 'g'))];
      expect(literals.length, `${struct} literals in ${file}`).toBeGreaterThan(0);
      expect(named.length, `${fields[0]} fields in ${list}`).toBe(literals.length);
      for (const name of commandsIn(body!, [...fields])) expect(commands).toContain(name);
    }
  });

  it('take a name from each field form, and no shell command for one', () => {
    const appDir = treeWith(KNOWN(['goose', 'new-agent-acp']), CHECKS);
    expect(agentCommands(appDir)).toEqual(['goose', 'new-agent-acp', 'pi', 'pi-acp', 'pi-old-acp']);
  });

  it('stop a run whose tree does not say, and one that names something that is no file name', () => {
    expect(() => agentCommands(join(scratch, 'nowhere', 'apps', 'staged'))).toThrow(
      /cannot read the agents the app knows from crates\/acp-client/
    );
    expect(() => agentCommands(treeWith('// moved elsewhere\n', CHECKS))).toThrow(
      /no KNOWN_AGENTS entries/
    );
    expect(() => agentCommands(treeWith(KNOWN(['goose']), 'const OTHER: u8 = 1;\n'))).toThrow(
      /no AI_AGENT_CHECKS entries/
    );
    expect(() => agentCommands(treeWith(KNOWN(['../bin/sh']), CHECKS))).toThrow(
      /not a plain file name/
    );
  });
});

describe('the stubs', () => {
  it('stand in the tools directory for every agent the resolver knows', () => {
    const { runDir, staged } = stagedRun();
    // The names as the Rust sources have them, not as the harness read them.
    const known = [
      ...readFileSync(join(REPO, 'crates/acp-client/src/types.rs'), 'utf8').matchAll(
        /^\s+command: "([^"]+)",$/gm
      ),
    ].map(([, name]) => name);
    const probed = [
      ...readFileSync(join(REPO, 'crates/doctor/src/agents.rs'), 'utf8').matchAll(
        /^\s+(?:commands: &\[|main_command: Some\()([^\])]+)[\])],$/gm
      ),
    ].flatMap(([, names]) => [...names.matchAll(/"([^"]+)"/g)].map(([, name]) => name));
    expect(known.length).toBeGreaterThanOrEqual(5);
    expect(probed.length).toBeGreaterThanOrEqual(known.length);
    for (const name of new Set([...known, ...probed])) {
      const file = join(runDir, 'acp-tools', name);
      expect(existsSync(file), `no stand-in for ${name} in the run's tools directory`).toBe(true);
      expect(statSync(file).mode & 0o111, `${name} is not executable`).not.toBe(0);
      // Its own file in the run directory, not a link to anywhere.
      expect(realpathSync(file)).toBe(file);
    }
    // The one a session runs is the fake agent; every other is a stub.
    expect(readFileSync(join(runDir, 'acp-tools', FAKE_AGENT_COMMAND), 'utf8')).toContain(
      'fake-agent.cjs'
    );
    expect(staged.agentStubs).not.toContain(FAKE_AGENT_COMMAND);
    expect([...staged.agentStubs, FAKE_AGENT_COMMAND].sort()).toEqual(agentCommands(APP_DIR));
  });

  it('cover an agent the tree gains without the harness being told', () => {
    const { runDir, staged } = stagedRun(treeWith(KNOWN(['goose', 'new-agent-acp']), CHECKS));
    expect(staged.agentStubs).toContain('new-agent-acp');
    expect(readFileSync(join(runDir, 'acp-tools', 'new-agent-acp'), 'utf8')).toContain(
      'new-agent-acp is a stub'
    );
  });

  it('record a start and fail at once, with nothing but the shell', () => {
    const { runDir, home, staged } = stagedRun();
    const run = (name: string, ...args: string[]) =>
      spawnSync(join(runDir, 'acp-tools', name), args, {
        // No PATH at all: a stub that needed a program would not find one.
        env: { HOME: home },
        encoding: 'utf8',
      });
    const goose = run('goose', 'acp', '--with-builtin', 'developer');
    expect(goose.status).toBe(1);
    expect(goose.stdout).toBe('');
    expect(goose.stderr).toContain('goose is a stub');
    expect(run('codex-acp').status).toBe(1);
    expect(run('goose', 'acp').status).toBe(1);
    const log = readFileSync(staged.agentStubLog, 'utf8');
    expect(log).toBe('goose acp --with-builtin developer\ncodex-acp \ngoose acp\n');
    expect(stubStarts(log)).toEqual({ goose: 2, 'codex-acp': 1 });
    expect(stubStarts('')).toEqual({});
  });
});

describe('the login shell the app asks', () => {
  const runDir = () => join(scratch, 'run');
  /** What a login shell under the run's HOME and environment searches. */
  function searched(shell: string, print: string) {
    const { home } = stagedRun();
    const out = spawnSync(shell, ['-l', '-c', print], {
      env: scrubbedEnv({ runDir: runDir(), home }),
      encoding: 'utf8',
    });
    return out.stdout.trim().split('\n').at(-1);
  }

  it('is given login files that put the run’s PATH back', () => {
    const files = loginFiles('/run');
    expect(Object.keys(files).sort()).toEqual([
      '.bash_profile',
      '.profile',
      '.zlogin',
      '.zprofile',
    ]);
    for (const text of Object.values(files))
      expect(text).toContain(`PATH='/run/bin:/usr/bin:/bin:/usr/sbin:/sbin'\nexport PATH\n`);
    const { home } = stagedRun();
    for (const name of Object.keys(files))
      expect(readFileSync(join(home, name), 'utf8')).toBe(loginFiles(runDir())[name]);
  });

  // The two lookups of `crates/doctor/src/resolve.rs`, with the shells it
  // names. A login shell rebuilds its PATH from the machine's files (on
  // macOS from `/etc/paths`, which can name the user's own directories), so
  // without the login files neither answer is the run's PATH.
  it.skipIf(!existsSync('/bin/zsh'))('searches the run’s PATH only, in zsh', () => {
    expect(searched('/bin/zsh', 'print -r -- "$PATH"')).toBe(runPath(runDir()));
  });

  it.skipIf(!existsSync('/bin/bash'))('searches the run’s PATH only, in bash', () => {
    expect(searched('/bin/bash', 'printf "%s\\n" "$PATH"')).toBe(runPath(runDir()));
  });

  it.skipIf(!existsSync('/bin/zsh'))(
    'does not find a program on a PATH the shell was started with',
    () => {
      // What the lookup is for: a name, found or not. The directory stands
      // for one the machine's files would add.
      const { home } = stagedRun();
      const elsewhere = join(scratch, 'elsewhere');
      mkdirSync(elsewhere);
      executable(join(elsewhere, 'some-agent'));
      const env = {
        ...scrubbedEnv({ runDir: runDir(), home }),
        PATH: `${elsewhere}:/usr/bin:/bin`,
      };
      const lookup = (withHome: string) =>
        spawnSync('/bin/zsh', ['-l', '-c', `whence -p -- 'some-agent'`], {
          env: { ...env, HOME: withHome },
          encoding: 'utf8',
        });
      expect(lookup(home).stdout.trim()).toBe('');
      expect(lookup(home).status).not.toBe(0);
      // Without the login files it is found, so the test would notice if
      // the shell stopped reading them.
      const bare = join(scratch, 'bare-home');
      mkdirSync(bare);
      expect(lookup(bare).stdout.trim().split('\n').at(-1)).toBe(join(elsewhere, 'some-agent'));
    }
  );
});

describe('what the app’s log says it started', () => {
  const line = (program: string, args = `'acp' '--with-builtin' 'developer'`) =>
    `[2026-11-25][07:37:52][acp_client::driver][INFO] Spawning Goose through login shell /run/bin/perf-shell (exec line on stdin); exec line: exec '${program}' ${args}`;

  it('is read in the form the app writes it', () => {
    // The format string in the app's source, filled in as `format!` would.
    const source = readFileSync(join(REPO, 'crates/acp-client/src/driver.rs'), 'utf8');
    expect(source).toContain(`"${SPAWN_LOG_FORMAT}"`);
    const values = ['Goose', '/bin/zsh', 'exec line on stdin', `exec '/opt/x/goose' 'acp'`];
    const written = SPAWN_LOG_FORMAT.replace(/\{\}/g, () => values.shift()!);
    expect(spawnsIn(`[INFO] ${written}\n`)).toEqual([
      { label: 'Goose', shell: '/bin/zsh', paths: ['/opt/x/goose'] },
    ]);
    expect(spawnLogProblem({ appDir: APP_DIR, profile: 'debug' })).toBeNull();
  });

  it('takes every path of an exec line, in each form the app writes one', () => {
    const log = [
      '[INFO] [create_project] spawning background setup for repo 1 in project 2',
      line('/Users/someone/.local/bin/goose'),
      // An interpreter the app found for a script, then the script.
      `[INFO] Spawning Codex through login shell /bin/zsh (exec line on stdin); exec line: exec '/run/bin/node' '/opt/homebrew/bin/codex-acp'`,
      // The guard for a script whose interpreter may not be on the PATH.
      `[INFO] Spawning Pi through login shell /bin/zsh (exec line on stdin); exec line: command -v 'node' >/dev/null 2>&1 || { if [ -n "$PATH" ]; then PATH='/opt/pi/bin':"$PATH"; else PATH='/opt/pi/bin'; fi; export PATH; }; exec '/opt/pi/bin/pi-acp' `,
      // Variables for the agent, and a quote in a path.
      `[INFO] Spawning Amp through login shell /bin/fish (non-POSIX shell, exec line through /bin/sh -c); exec line: set -- 'KEY='"$__STAGED_EXTRA_ENV_0"; unset __STAGED_EXTRA_ENV_0; exec /usr/bin/env -- "$@" '/Users/o'\\''brien/bin/amp-acp' 'x'`,
      '[WARN] Action detection failed for local/perf-view-0',
    ].join('\n');
    expect(spawnsIn(log)).toEqual([
      { label: 'Goose', shell: '/run/bin/perf-shell', paths: ['/Users/someone/.local/bin/goose'] },
      {
        label: 'Codex',
        shell: '/bin/zsh',
        paths: ['/run/bin/node', '/opt/homebrew/bin/codex-acp'],
      },
      {
        label: 'Pi',
        shell: '/bin/zsh',
        paths: ['/opt/pi/bin', '/opt/pi/bin', '/opt/pi/bin/pi-acp'],
      },
      { label: 'Amp', shell: '/bin/fish', paths: [`/Users/o'brien/bin/amp-acp`] },
    ]);
  });

  it('is followed as the log grows, a line at a time', () => {
    const file = join(scratch, 'app.log');
    const spawns = followSpawns(file);
    expect(spawns.read()).toEqual([]);
    // A line cut in the middle of a character, as a read can find it.
    const whole = Buffer.from(`${line('/opt/é/goose')}\n`);
    const cut = whole.indexOf(0xc3) + 1;
    writeFileSync(file, whole.subarray(0, cut));
    expect(spawns.read()).toEqual([]);
    appendFileSync(file, whole.subarray(cut));
    expect(spawns.read().map((s) => s.paths)).toEqual([['/opt/é/goose']]);
    expect(spawns.read()).toEqual([]);
    appendFileSync(file, `${line('/run/acp-tools/goose')}\n${line('/usr/local/bin/goose')}\n`);
    expect(spawns.read().map((s) => s.paths)).toEqual([
      ['/run/acp-tools/goose'],
      ['/usr/local/bin/goose'],
    ]);
  });

  it('cannot be read from a release build, or from a tree that logs otherwise', () => {
    expect(spawnLogProblem({ appDir: APP_DIR, profile: 'release' })).toMatch(
      /release build of the app writes no log/
    );
    expect(
      spawnLogProblem({ appDir: treeWith(KNOWN(['goose']), CHECKS), profile: 'debug' })
    ).toMatch(/does not log an agent start/);
  });

  it('fails the run for a start outside the run directory, by name, and only for one', () => {
    const runDir = join(scratch, 'run');
    mkdirSync(join(runDir, 'acp-tools'), { recursive: true });
    mkdirSync(join(runDir, 'logs'));
    executable(join(runDir, 'acp-tools', 'goose'));
    const appLog = join(runDir, 'logs', 'app.log');
    const stubLog = join(runDir, 'logs', 'agent-stubs.log');
    const programs = watchPrograms({
      appLog,
      expected: expectedPlaces({ runDir }),
      stubbed: ['goose'],
      stubLog,
    });
    const sampled = {
      processSamples: 4,
      openFileSamples: 2,
      openUnderRealStaged: [],
      unexpectedExecutables: [],
    };
    const perfLine = (program: string) => line(program).replace('/run/bin', `${runDir}/bin`);

    // Nothing started, then the stub twice: a run that is as it should be.
    expect(programs.safety(sampled).agents).toEqual({
      stubbed: ['goose'],
      stubStarts: {},
      startsLogged: 0,
      startsLoggedOutside: 0,
    });
    appendFileSync(appLog, `${perfLine(join(runDir, 'acp-tools', 'goose'))}\n`.repeat(2));
    writeFileSync(stubLog, 'goose acp\ngoose acp\n');
    let safety = programs.safety(sampled);
    expect(safety.unexpectedExecutables).toEqual([]);
    expect(safety.agents).toMatchObject({
      stubStarts: { goose: 2 },
      startsLogged: 2,
      startsLoggedOutside: 0,
    });
    expect(outsideProgramsFailure(safety.unexpectedExecutables)).toBeNull();

    // The user's own agent, as the defect had it, three times.
    appendFileSync(appLog, `${perfLine('/Users/someone/.local/bin/goose')}\n`.repeat(3));
    safety = programs.safety({ ...sampled, unexpectedExecutables: ['/opt/other/tool --flag'] });
    expect(safety.unexpectedExecutables).toEqual([
      '/Users/someone/.local/bin/goose (started by the app as Goose, from its log), 3 times',
      '/opt/other/tool --flag',
    ]);
    expect(safety.agents).toMatchObject({ startsLogged: 5, startsLoggedOutside: 3 });
    expect(safety.processSamples).toBe(4);
    const failure = outsideProgramsFailure(safety.unexpectedExecutables)!;
    expect(failure.message).toBe(
      'the run started 2 programs from outside the run directory and the system directories: /Users/someone/.local/bin/goose (started by the app as Goose, from its log), 3 times; /opt/other/tool --flag'
    );
    expect(failure.detail).toEqual({
      kind: 'outside-program',
      programs: safety.unexpectedExecutables,
    });
    // It stays found: a later look at the same log reports it again.
    expect(programs.safety(sampled).unexpectedExecutables).toHaveLength(1);
  });

  it('counts a file in the run directory that leads outside it as outside', () => {
    const runDir = join(scratch, 'run');
    mkdirSync(join(runDir, 'acp-tools'), { recursive: true });
    mkdirSync(join(runDir, 'logs'));
    const real = join(scratch, 'elsewhere-goose');
    executable(real);
    symlinkSync(real, join(runDir, 'acp-tools', 'goose'));
    const appLog = join(runDir, 'logs', 'app.log');
    writeFileSync(appLog, `${line(join(runDir, 'acp-tools', 'goose'))}\n`);
    const programs = watchPrograms({
      appLog,
      expected: expectedPlaces({ runDir }),
      stubbed: [],
      stubLog: join(runDir, 'logs', 'agent-stubs.log'),
    });
    const found = programs.safety({
      processSamples: 0,
      openFileSamples: 0,
      openUnderRealStaged: [],
      unexpectedExecutables: [],
    }).unexpectedExecutables;
    expect(found).toEqual([
      `${real} (through ${join(runDir, 'acp-tools', 'goose')}) (started by the app as Goose, from its log)`,
    ]);
    expect(outsideProgramsFailure(found)!.message).toContain(real);
  });

  it('says nothing of starts when the build does not log them', () => {
    const programs = watchPrograms({
      appLog: join(scratch, 'none.log'),
      expected: expectedPlaces({ runDir: scratch }),
      stubbed: ['goose'],
      stubLog: join(scratch, 'none-stubs.log'),
      logProblem: 'a release build of the app writes no log',
    });
    expect(
      programs.safety({
        processSamples: 0,
        openFileSamples: 0,
        openUnderRealStaged: [],
        unexpectedExecutables: [],
      }).agents
    ).toEqual({
      stubbed: ['goose'],
      stubStarts: {},
      startsLogged: null,
      startsLoggedOutside: null,
    });
  });
});

describe('a process’s program', () => {
  const expected = expectedPlaces({
    runDir: '/run',
    allowed: ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'],
  });
  /** A file system of executables (`path: real path`) and plain files. */
  const files = (executables: Record<string, string>, plain: string[] = []) => ({
    real: (path: string) => executables[path] ?? (plain.includes(path) ? path : null),
    executable: (path: string) => path in executables,
  });
  const none = files({});

  it('is fine in the run directory, the system and the browser named', () => {
    for (const args of [
      '/run/target/debug/Staged',
      '/run/bin/node /run/harness/agent/fake-agent.cjs --config /run/agent-config.json',
      '/usr/bin/git -C /run/home/.staged/repos/local/perf-view-0 status',
      '/Library/Developer/CommandLineTools/usr/bin/git fsmonitor--daemon run',
      '/bin/zsh -l -c whence -p -- goose',
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --headless about:blank',
      'git status',
      '(node)',
    ])
      expect(outsideProgram(args, expected, none), args).toBeNull();
  });

  it('is named when it is anywhere else', () => {
    expect(outsideProgram('/Users/someone/.local/bin/goose acp', expected, none)).toBe(
      '/Users/someone/.local/bin/goose acp'
    );
    expect(outsideProgram('/opt/homebrew/bin/sq blox list', expected, none)).toBe(
      '/opt/homebrew/bin/sq blox list'
    );
  });

  it('is the script when the program is only what runs it', () => {
    const fs = files({
      '/opt/homebrew/bin/codex-acp': '/opt/homebrew/lib/node_modules/codex-acp/dist/index.js',
      '/run/harness/agent/fake-agent.cjs': '/run/harness/agent/fake-agent.cjs',
      '/run/acp-tools/goose': '/tmp/elsewhere/goose',
      '/run/bin/node': '/run/bin/node',
    });
    // The run's own Node, a bare name found on the PATH, and the shell.
    expect(outsideProgram('/run/bin/node /opt/homebrew/bin/codex-acp', expected, fs)).toBe(
      '/opt/homebrew/bin/codex-acp, run by /run/bin/node'
    );
    expect(outsideProgram('node --no-warnings /opt/homebrew/bin/codex-acp cli', expected, fs)).toBe(
      '/opt/homebrew/bin/codex-acp, run by node'
    );
    // A script in the run directory that is a link to one outside it.
    expect(outsideProgram('/bin/sh /run/acp-tools/goose acp', expected, fs)).toBe(
      '/tmp/elsewhere/goose (through /run/acp-tools/goose), run by /bin/sh'
    );
    expect(outsideProgram('/run/acp-tools/goose acp', expected, fs)).toBe(
      '/tmp/elsewhere/goose (through /run/acp-tools/goose)'
    );
    expect(
      outsideProgram('/run/bin/node /run/harness/agent/fake-agent.cjs', expected, fs)
    ).toBeNull();
  });

  it('is not a file a system tool was only given', () => {
    const fs = files({}, ['/etc/paths', '/Users/someone/notes.txt']);
    expect(outsideProgram('/bin/cat /etc/paths', expected, fs)).toBeNull();
    expect(outsideProgram('/usr/bin/git -C /Users/someone/repo status', expected, fs)).toBeNull();
    expect(outsidePlace('/Users/someone/notes.txt', expected, fs.real)).toBe(
      '/Users/someone/notes.txt'
    );
  });

  it('is checked against real files by default', () => {
    const runDir = join(scratch, 'run');
    mkdirSync(join(runDir, 'acp-tools'), { recursive: true });
    const real = join(scratch, 'outside-script');
    executable(real);
    writeFileSync(join(scratch, 'data.txt'), 'x');
    symlinkSync(real, join(runDir, 'acp-tools', 'goose'));
    const places = expectedPlaces({ runDir });
    expect(outsideProgram(`/bin/sh ${join(runDir, 'acp-tools', 'goose')} acp`, places)).toBe(
      `${real} (through ${join(runDir, 'acp-tools', 'goose')}), run by /bin/sh`
    );
    expect(outsideProgram(`/bin/sh ${real}`, places)).toBe(`${real}, run by /bin/sh`);
    expect(outsideProgram(`/bin/cat ${join(scratch, 'data.txt')}`, places)).toBeNull();
    expect(outsideProgram(`/bin/sh ${join(scratch, 'missing')}`, places)).toBeNull();
  });
});

describe('the summary', () => {
  const result = (agents?: object) => ({
    label: 'run',
    tree: { head: 'abcdef0123456789', branch: 'main', dirty: false },
    frontend: 'browser',
    streamSupported: true,
    params: parseParams([]),
    host: { loadAverageStart: [1, 1, 1], loadAverageEnd: [1, 1, 1] },
    complete: true,
    failure: null,
    notRun: [],
    warnings: [],
    checks: { ok: true, failed: [] },
    scenarios: {},
    safety: {
      runDir: '/run',
      dataDir: '/run/home/.staged',
      realStagedDir: '/home/.staged',
      openFileSamples: 3,
      openUnderRealStaged: [],
      unexpectedExecutables: [],
      leftoverProcesses: [],
      ...(agents && { agents }),
    },
  });

  it('says which agents were stubbed, how often one was started, and what the log had', () => {
    const summary = summarize(
      result({
        stubbed: ['amp-acp', 'goose'],
        stubStarts: { goose: 5 },
        startsLogged: 5,
        startsLoggedOutside: 0,
      })
    );
    expect(summary).toContain(
      "- agents stubbed in the run's tools directory: amp-acp, goose; stub starts: 5 (goose 5)"
    );
    expect(summary).toContain(
      "- agent starts in the app's log: 5, 0 of them outside the run directory"
    );
  });

  it('says when the log was not read, and nothing for a result from before the stubs', () => {
    const summary = summarize(
      result({ stubbed: ['goose'], stubStarts: {}, startsLogged: null, startsLoggedOutside: null })
    );
    expect(summary).toContain('; stub starts: 0\n');
    expect(summary).toContain("- agent starts in the app's log: not read");
    expect(summarize(result())).not.toContain('agents stubbed');
  });
});
