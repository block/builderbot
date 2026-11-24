#!/usr/bin/env node
// Fake ACP agent for the load-test harness. Speaks the Agent Client Protocol
// over stdio as `claude-agent-acp` does, with no model, no network, no
// credentials and no file access beyond its config and a counter file.
//
//   fake-agent.cjs --config <config.json> --state <dir>
//
// A prompt names its load with two words: `pf-tag=<tag>` (lower-case letters
// and digits, unique per session) and `pf-profile=<name>` (a key of the
// config's `profiles`). A prompt with no tag (a reminder Staged sends by
// itself, a title request) gets one line back.
//
// Every piece of output carries a stamp, `pfm-<kind><tag>-<seq>-<epoch ms>`,
// taken as the piece is written to stdout: kind `x` for text, `t` for a tool
// call. The app's harness build reads it when the row is stored and the page
// probe reads it when it reaches the screen.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const readline = require('node:readline');

const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(name);
  return i === -1 ? undefined : argv[i + 1];
};
const config = JSON.parse(fs.readFileSync(flag('--config'), 'utf8'));
const stateDir = flag('--state');

const send = (message) =>
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n');
const update = (sessionId, body) =>
  send({ method: 'session/update', params: { sessionId, update: body } });
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const sessions = new Map(); // ACP session id -> { cwd, cancel }
let sessionCounter = 0;

const WORDS = 'the quick brown fox jumps over the lazy dog while the build keeps running '.split(
  ' '
);
function filler(chars, salt) {
  let out = '';
  for (let i = salt; out.length < chars; i++) out += WORDS[i % (WORDS.length - 1)] + ' ';
  return out.slice(0, chars).trimEnd();
}
function body(bytes, label) {
  const line = `${label}: the quick brown fox jumps over the lazy dog\n`;
  return line.repeat(Math.ceil(bytes / line.length)).slice(0, bytes);
}

/** One turn's counters, written where the driver can read them. */
function counters(tag) {
  const state = {
    tag,
    pid: process.pid,
    startedAt: Date.now(),
    chunks: 0,
    toolCalls: 0,
    plans: 0,
    bytes: 0,
    seq: 0,
    endedAt: null,
    stop: null,
  };
  const file = stateDir && path.join(stateDir, `${tag}.json`);
  const write = () => {
    if (file) fs.writeFileSync(file, JSON.stringify(state));
  };
  const timer = setInterval(write, 1000);
  timer.unref();
  return { state, write, done: () => (clearInterval(timer), write()) };
}

async function toolCall(sessionId, cwd, profile, turn, cancelled) {
  const { state } = turn;
  state.toolCalls += 1;
  const n = state.toolCalls;
  const large = profile.largeToolEvery > 0 && n % profile.largeToolEvery === 0;
  const bytes = large ? profile.largeToolBytes : profile.toolBytes;
  const toolCallId = `tool-${state.tag}-${n}`;
  const stamp = () => `pfm-t${state.tag}-${++state.seq}-${Date.now()}`;
  const read = n % 2 === 0;
  update(sessionId, {
    sessionUpdate: 'tool_call',
    toolCallId,
    title: read ? 'Read' : 'Bash',
    kind: read ? 'read' : 'execute',
    status: 'in_progress',
    rawInput: read
      ? { file_path: `${cwd}/src/${stamp()}.ts` }
      : { command: `run ${stamp()}`, description: `Run ${stamp()}` },
  });
  // In flight: the output so far is on the row, as a real agent's is, and
  // grows until the call settles.
  const steps = Math.max(1, Math.round(profile.toolMs / 500));
  for (let step = 1; step <= steps && profile.toolMs > 0 && !cancelled(); step++) {
    const text = body(Math.ceil((bytes * step) / steps), toolCallId) + `\n${stamp()}`;
    state.bytes += text.length;
    update(sessionId, {
      sessionUpdate: 'tool_call_update',
      toolCallId,
      status: 'in_progress',
      content: [{ type: 'content', content: { type: 'text', text } }],
    });
    await sleep(profile.toolMs / steps);
  }
  const text = body(bytes, toolCallId) + `\n${stamp()}`;
  state.bytes += text.length;
  update(sessionId, {
    sessionUpdate: 'tool_call_update',
    toolCallId,
    status: cancelled() ? 'failed' : 'completed',
    content: [{ type: 'content', content: { type: 'text', text } }],
    rawOutput: { ok: true, id: toolCallId },
  });
}

async function runTurn(id, sessionId, prompt) {
  const session = sessions.get(sessionId) ?? { cwd: process.cwd() };
  sessions.set(sessionId, session);
  const text = (prompt ?? []).map((block) => (block.type === 'text' ? block.text : '')).join('\n');
  const tag = (text.match(/pf-tag=([a-z0-9]+)/) ?? [])[1];
  const profile = config.profiles[(text.match(/pf-profile=([\w-]+)/) ?? [])[1]];
  if (!tag || !profile) {
    update(sessionId, {
      sessionUpdate: 'agent_message_chunk',
      content: { type: 'text', text: 'Nothing to do.' },
    });
    return send({ id, result: { stopReason: 'end_turn' } });
  }
  let cancelled = false;
  session.cancel = () => (cancelled = true);
  const turn = counters(tag);
  const { state } = turn;
  update(sessionId, { sessionUpdate: 'session_info_update', title: `Load ${tag}` });
  const interval = 1000 / profile.chunksPerSecond;
  const deadline = state.startedAt + profile.turnSeconds * 1000;
  let next = Date.now();
  while (Date.now() < deadline && !cancelled) {
    state.chunks += 1;
    const k = state.chunks;
    if (profile.toolEvery > 0 && k % profile.toolEvery === 0) {
      await toolCall(sessionId, session.cwd, profile, turn, () => cancelled);
      next = Date.now();
      continue;
    }
    if (profile.planEvery > 0 && k % profile.planEvery === 0) {
      state.plans += 1;
      update(sessionId, {
        sessionUpdate: 'plan',
        entries: [1, 2, 3].map((step) => ({
          content: `Step ${step} of plan ${state.plans}`,
          priority: 'medium',
          status: step === 1 ? 'in_progress' : 'pending',
        })),
      });
      update(sessionId, {
        sessionUpdate: 'session_info_update',
        title: `Load ${tag} (${state.plans})`,
      });
    }
    // One line per chunk, so the newest line (what a subtitle shows) always
    // begins with the newest stamp.
    const lead =
      k === 1 ? '' : profile.paragraphEvery > 0 && k % profile.paragraphEvery === 0 ? '\n\n' : '\n';
    const chunk = `${lead}pfm-x${tag}-${++state.seq}-${Date.now()} ${filler(profile.chunkChars, k)}`;
    state.bytes += chunk.length;
    update(sessionId, {
      sessionUpdate: 'agent_message_chunk',
      content: { type: 'text', text: chunk },
    });
    // Against the clock rather than a fixed sleep, so the rate holds when a
    // timer fires late under load.
    next += interval;
    const wait = next - Date.now();
    if (wait > 0) await sleep(wait);
    else next = Date.now();
  }
  if (!cancelled)
    update(sessionId, {
      sessionUpdate: 'agent_message_chunk',
      content: { type: 'text', text: `\n\nFinished ${tag}.` },
    });
  state.endedAt = Date.now();
  state.stop = cancelled ? 'cancelled' : 'end_turn';
  turn.done();
  session.cancel = null;
  send({ id, result: { stopReason: state.stop } });
}

readline.createInterface({ input: process.stdin }).on('line', (line) => {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return;
  }
  const { id, method, params } = message;
  switch (method) {
    case undefined:
      return; // a response to something this agent never asks
    case 'initialize':
      return send({
        id,
        result: {
          protocolVersion: 1,
          agentCapabilities: {
            loadSession: true,
            promptCapabilities: { image: true, audio: false, embeddedContext: true },
            mcpCapabilities: { http: true, sse: true },
          },
          authMethods: [],
          agentInfo: { name: 'staged-perf-fake-agent', version: '1' },
        },
      });
    case 'authenticate':
      return send({ id, result: {} });
    case 'session/new': {
      const sessionId = `fake-${process.pid}-${++sessionCounter}`;
      sessions.set(sessionId, { cwd: params.cwd });
      return send({ id, result: { sessionId } });
    }
    case 'session/load':
      sessions.set(params.sessionId, { cwd: params.cwd });
      return send({ id, result: {} });
    case 'session/prompt':
      return void runTurn(id, params.sessionId, params.prompt);
    case 'session/cancel':
      return void sessions.get(params.sessionId)?.cancel?.();
    case 'session/set_mode':
    case 'session/set_model':
      return send({ id, result: {} });
    case 'session/set_config_option':
      return send({ id, result: { configOptions: [] } });
    default:
      if (id !== undefined)
        send({ id, error: { code: -32601, message: `Method not found: ${method}` } });
  }
});
// The app closing the pipe is the only way this process ends; it never
// outlives the app.
process.stdin.on('close', () => process.exit(0));
process.stdin.on('end', () => process.exit(0));
process.stdout.on('error', () => process.exit(0));
