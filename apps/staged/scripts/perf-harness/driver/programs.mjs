// Whether a run has started a program from outside its own directory and
// the system's, from the two places that can tell: the process sampler
// (`sampler.mjs`), which sees what is alive at one of its samples, and the
// app's log (`lib/spawns.mjs`), which names every agent the app starts
// through the login shell before it starts it, however briefly it lives.
// Also what the stubs standing in for the user's agents recorded.

import { existsSync, readFileSync } from 'node:fs';

import { stubStarts } from '../lib/agents.mjs';
import { outsidePlace } from '../lib/safety.mjs';
import { followSpawns } from '../lib/spawns.mjs';

/**
 * `expected` are the places a program may be (`expectedPlaces`), `stubbed`
 * the agents stubbed and `stubLog` where the stubs write. `logProblem` is
 * why the app's log holds no starts, when it does not.
 */
export function watchPrograms({ appLog, expected, stubbed, stubLog, logProblem = null }) {
  const spawnLog = followSpawns(appLog);
  let total = 0;
  const outside = new Map();

  function readLog() {
    for (const { label, paths } of spawnLog.read()) {
      total += 1;
      const found = paths.map((path) => outsidePlace(path, expected)).filter(Boolean);
      // One entry per start, named by the agent's own path, the last on its line.
      if (found.length) {
        const program = `${found.at(-1)} (started by the app as ${label}, from its log)`;
        outside.set(program, (outside.get(program) ?? 0) + 1);
      }
    }
  }

  return {
    /**
     * The result's safety fields, from the sampler's (`sampled`) and
     * whatever the app has logged since the last call.
     */
    safety(sampled) {
      readLog();
      return {
        ...sampled,
        unexpectedExecutables: [
          ...[...outside].map(([program, times]) =>
            times > 1 ? `${program}, ${times} times` : program
          ),
          ...sampled.unexpectedExecutables,
        ],
        agents: {
          stubbed,
          stubStarts: existsSync(stubLog) ? stubStarts(readFileSync(stubLog, 'utf8')) : {},
          // `null` when this build's log does not record them.
          startsLogged: logProblem ? null : total,
          startsLoggedOutside: logProblem
            ? null
            : [...outside.values()].reduce((sum, times) => sum + times, 0),
        },
      };
    },
  };
}

/** What to fail a run with for the programs it should not have started, or `null` for none. */
export function outsideProgramsFailure(programs) {
  if (programs.length === 0) return null;
  return {
    message: `the run started ${programs.length === 1 ? 'a program' : `${programs.length} programs`} from outside the run directory and the system directories: ${programs.join('; ')}`,
    detail: { kind: 'outside-program', programs },
  };
}
