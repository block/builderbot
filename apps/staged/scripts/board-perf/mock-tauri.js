// In-page stand-in for the Tauri IPC bridge, for the board performance
// harness (run.mjs). Installed before the app's own scripts, it makes the
// production frontend take its desktop code path (`isTauri` true: no
// IndexedDB cache, no HTTP) and answers every `invoke` from the fixture the
// backend benchmark wrote (board_load_bench.rs), so no Staged backend and
// no real data are involved.
//
// Each reply is parsed from JSON afresh and delivered in a task of its own,
// as a real IPC reply is: the app sees new objects per call and renders
// between replies. Replies carry no backend latency, so what the harness
// times is the frontend's own work.
(() => {
  const input = window.__BOARD_PERF_INPUT__;
  const fixture = input.fixture;
  const prefs = new Map(Object.entries(input.prefs));

  const state = {
    counts: {},
    misses: {},
    pending: 0,
    lastReplyAt: 0,
    firstCardAt: null,
    allCardsAt: null,
    lastBusyEnd: 0,
    blockedMs: 0,
    longestFrameMs: 0,
    frames: 0,
    // Projects whose branch list the app has been given since the last reset.
    hydrated: 0,
  };
  let hydratedIds = new Set();
  const served = (projectId) => {
    hydratedIds.add(projectId);
    state.hydrated = hydratedIds.size;
  };

  // One task per reply, with no timer clamping.
  const channel = new MessageChannel();
  const replies = [];
  channel.port1.onmessage = () => replies.shift()?.();
  const inOwnTask = (fn) => {
    replies.push(fn);
    channel.port2.postMessage(0);
  };

  const json = new Map();
  const fresh = (key, value) => {
    let text = json.get(key);
    if (text === undefined) {
      text = JSON.stringify(value ?? null);
      json.set(key, text);
    }
    return JSON.parse(text);
  };

  const callbacks = new Map();
  let nextCallbackId = 1;
  const listeners = new Map();

  const commands = {
    list_projects: () => fresh('projects', fixture.projects),
    list_branches_for_project: ({ projectId }) => {
      served(projectId);
      return fresh(`b:${projectId}`, fixture.branches[projectId] ?? []);
    },
    list_project_repos: ({ projectId }) => fresh(`r:${projectId}`, fixture.repos[projectId] ?? []),
    // The bulk reads the app hydrates many projects with: one map of project
    // id to list each, which is how the fixture stores them.
    list_branches_by_project: () => {
      for (const projectId of Object.keys(fixture.branches)) served(projectId);
      return fresh('b:*', fixture.branches);
    },
    list_project_repos_by_project: () => fresh('r:*', fixture.repos),
    get_tag_state: () => fresh('tags', fixture.tagState),
    get_all_repo_badges: () => fresh('badges', fixture.badges),
    ensure_repo_badges: () => [],
    list_repos_for_home: () =>
      fresh(
        'home',
        fixture.badges.filter((badge) => badge.pinned)
      ),
    preferences_store_path: () => '/board-perf/preferences.json',
    get_project_search_index: () => [],
    get_active_sessions: () => [],
    get_all_running_actions: () => [],
    list_all_repo_actions: () => [],
    list_action_contexts: () => [],
    list_project_actions: () => [],
    list_project_notes: () => [],
    list_transferring_projects: () => [],
    get_running_branch_actions: () => [],
    poll_all_workspace_statuses: () => [],
    discover_acp_providers: () => [],
    get_suggested_repos: () => [],
    get_branch_timeline: () => ({
      commits: [],
      notes: [],
      reviews: [],
      images: [],
      gitState: null,
    }),
    refresh_branch_git_state: () => null,
    is_sq_available: () => false,
    get_store_status: () => null,
    get_blox_env: () => null,
    get_interrupted_sessions: () => null,
    get_session_reconcile_states: () => [],
    get_workspace_setup_errors: () => ({}),
    list_pending_control_authorizations: () => [],
    list_pending_project_opens: () => [],
    set_foreground_project: () => null,
    set_focus: () => null,
    note_window_revealed: () => null,
    set_project_group_tag: ({ projectId, groupId, tagId }) => {
      const inGroup = new Set(
        fixture.tagState.tags.filter((tag) => tag.groupId === groupId).map((tag) => tag.id)
      );
      fixture.tagState.assignments = fixture.tagState.assignments.filter(
        (a) => !(a.projectId === projectId && inGroup.has(a.tagId))
      );
      if (tagId) fixture.tagState.assignments.push({ projectId, tagId });
      json.delete('tags');
      inOwnTask(() => api.emit('tags-changed', { projectIds: [projectId] }));
      return null;
    },
    // Tauri plugins the app talks to at startup.
    'plugin:store|load': () => 1,
    'plugin:store|get_store': () => 1,
    'plugin:store|get': ({ key }) => [prefs.get(key) ?? null, prefs.has(key)],
    'plugin:store|has': ({ key }) => prefs.has(key),
    'plugin:store|set': ({ key, value }) => void prefs.set(key, value),
    'plugin:store|delete': ({ key }) => prefs.delete(key),
    'plugin:store|keys': () => [...prefs.keys()],
    'plugin:store|entries': () => [...prefs.entries()],
    'plugin:store|save': () => null,
    'plugin:event|listen': ({ event, handler }) => {
      if (!listeners.has(event)) listeners.set(event, []);
      listeners.get(event).push(handler);
      return handler;
    },
    'plugin:event|unlisten': ({ event, eventId }) => {
      const list = listeners.get(event) ?? [];
      const index = list.indexOf(eventId);
      if (index !== -1) list.splice(index, 1);
      return null;
    },
  };

  function answer(cmd, args) {
    const handler = commands[cmd];
    if (handler) return handler(args ?? {});
    // Anything else gets null. App commands that reach here are listed in
    // `misses` so the fixture can be completed; plugin calls are expected.
    if (!cmd.startsWith('plugin:')) state.misses[cmd] = (state.misses[cmd] ?? 0) + 1;
    return null;
  }

  function invoke(cmd, args) {
    state.counts[cmd] = (state.counts[cmd] ?? 0) + 1;
    state.pending++;
    return new Promise((resolve, reject) => {
      inOwnTask(() => {
        state.pending--;
        state.lastReplyAt = performance.now();
        try {
          resolve(answer(cmd, args));
        } catch (error) {
          reject(error);
        }
      });
    });
  }

  window.__TAURI__ = {};
  window.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback(callback, once = false) {
      const id = nextCallbackId++;
      callbacks.set(id, (data) => {
        if (once) callbacks.delete(id);
        return callback?.(data);
      });
      return id;
    },
    unregisterCallback: (id) => void callbacks.delete(id),
    runCallback: (id, data) => callbacks.get(id)?.(data),
    callbacks,
    convertFileSrc: (path) => path,
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { windowLabel: 'main', label: 'main' },
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (_event, id) => void callbacks.delete(id),
  };

  // Frame watcher: when the cards first show, when they are all there, and
  // how long the main thread kept frames from running.
  const cardSelector = '[data-board-card], .project-card';
  let lastFrame = performance.now();
  function frame(now) {
    const gap = now - lastFrame;
    lastFrame = now;
    state.frames++;
    if (gap > 50) {
      state.blockedMs += gap;
      state.lastBusyEnd = now;
      if (gap > state.longestFrameMs) state.longestFrameMs = gap;
    }
    if (state.allCardsAt === null) {
      const count = document.querySelectorAll(cardSelector).length;
      if (count > 0 && state.firstCardAt === null) state.firstCardAt = now;
      if (count >= input.expectedCards) state.allCardsAt = now;
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);

  const api = {
    state,
    fixture,
    /** Deliver a backend event to the app's listeners. */
    emit(event, payload) {
      for (const handler of listeners.get(event) ?? []) {
        callbacks.get(handler)?.({ event, id: handler, payload });
      }
    },
    /** Forget the cached JSON after changing the fixture in place. */
    invalidate: () => json.clear(),
    /** Start a new measurement window. */
    reset() {
      state.counts = {};
      state.blockedMs = 0;
      state.longestFrameMs = 0;
      state.lastBusyEnd = 0;
      state.lastReplyAt = 0;
      hydratedIds = new Set();
      state.hydrated = 0;
      return performance.now();
    },
    snapshot: () => ({ ...state, counts: { ...state.counts }, now: performance.now() }),
  };
  window.__boardPerf = api;
})();
