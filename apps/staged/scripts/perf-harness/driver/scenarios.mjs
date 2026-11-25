// The scenarios: which sessions stream and what the page shows while a
// window is measured. Every scenario views the first of the projects it is
// given, which the driver seeds afresh for it; the other projects' sessions
// are load the page does not look at.

export const SCENARIOS = {
  subtitles: {
    about: 'a project detail with many running sessions, no chat open',
    chat: false,
    hidden: false,
    quietProject: false,
  },
  chat: {
    about: 'an open chat on a fast-streaming session, the only one running in its project',
    chat: true,
    hidden: false,
    quietProject: true,
  },
  both: {
    about:
      'an open chat on a fast-streaming session over a project detail with many running sessions',
    chat: true,
    hidden: false,
    quietProject: false,
  },
  hidden: {
    about: 'the same view as `both`, with the window hidden while it is measured',
    chat: true,
    hidden: true,
    quietProject: false,
  },
};

/**
 * The sessions one scenario starts: `[{ project, branch, tag, profile }]`.
 * Slots are filled project by project in turn, so `running` sessions are
 * spread as evenly as the number allows.
 */
export function plan({ name, index, projects, running }) {
  const scenario = SCENARIOS[name];
  const slots = [];
  const most = Math.max(...projects.map((p) => p.branches.length));
  for (let b = 0; b < most; b++) {
    for (let p = 0; p < projects.length; p++) {
      if (projects[p].branches[b]) slots.push({ project: p, branch: b });
    }
  }
  return slots
    .slice(0, running > 0 ? running : slots.length)
    .filter((slot) => !(scenario.quietProject && slot.project === 0 && slot.branch > 0))
    .map((slot) => ({
      ...slot,
      branchId: projects[slot.project].branches[slot.branch].id,
      tag: `s${index}p${slot.project}b${slot.branch}`,
      profile: scenario.chat && slot.project === 0 && slot.branch === 0 ? 'chat' : 'background',
    }));
}
