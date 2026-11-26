// What the driver does in the app's UI, as scripts run inside the page: the
// one place that knows the app's markup. A tree whose markup differs needs
// only these selectors changed.
//
// Each export returns the body of an async function for `frontend.evaluate`.
//
// A session is found by its branch's card (`data-branch-id`), not by
// searching the page for its row: a card mounts its timeline only while it
// is within about one and a half viewports of the scroll area, so in a
// project with more sessions than fit there, most rows do not exist until
// their card is scrolled to.

const HELPERS = `
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const visible = (el) => !el.closest('[aria-hidden="true"]');
  const waitFor = async (what, find, timeout = 45000) => {
    const end = Date.now() + timeout;
    for (;;) {
      const found = find();
      if (found) return found;
      if (Date.now() > end) throw new Error('not found in the page: ' + what);
      await sleep(100);
    }
  };
  // A parked copy of a page stays in the DOM under aria-hidden; the live one wins.
  const cardOf = (branchId) => {
    const cards = [...document.querySelectorAll('.branch-card[data-branch-id="' + branchId + '"]')];
    return cards.find(visible) ?? cards[0] ?? null;
  };
  const STAMP = /pfm-([xt])([a-z0-9]+)-(\\d+)-(\\d{13})/g;
  // The session's row in its card: the one that names the tag in its prompt
  // or in a stamp. A tag is matched whole, so s0p0b1 is not found in s0p0b10.
  const rowIn = (card, tag) => {
    const named = new RegExp('(?:pf-tag=|pfm-[xt])' + tag + '(?![a-z0-9])');
    return [...card.querySelectorAll('.timeline-row')].find((row) => named.test(row.textContent)) ?? null;
  };
  // Brings a session's card to the middle of the scroll area and waits for its row.
  const rowOf = (branchId, tag) =>
    waitFor('the row of session ' + tag, () => {
      const card = cardOf(branchId);
      if (!card) return null;
      const row = rowIn(card, tag);
      if (!row) card.scrollIntoView({ block: 'center' });
      return row;
    });
`;

/** Open a project's detail from wherever the page is, and wait for a session's row. */
export const openProject = (projectName, { branchId, tag }) => `${HELPERS}
  if (!cardOf(${JSON.stringify(branchId)})) {
    const name = await waitFor('the project ${projectName}', () =>
      [...document.querySelectorAll('*')].find((el) => el.children.length === 0 && visible(el) && el.textContent.trim() === ${JSON.stringify(projectName)}));
    name.click();
    await waitFor('the card of branch ${branchId}', () => cardOf(${JSON.stringify(branchId)}));
  }
  const row = await rowOf(${JSON.stringify(branchId)}, ${JSON.stringify(tag)});
  row.scrollIntoView({ block: 'center' });
  return { rows: document.querySelectorAll('.timeline-row').length };
`;

/** Open a session's chat from its row, and wait for its transcript. */
export const openChat = ({ branchId, tag }) => `${HELPERS}
  const row = await rowOf(${JSON.stringify(branchId)}, ${JSON.stringify(tag)});
  const button = await waitFor('the View session button', () => row.querySelector('button[aria-label="View session"]'));
  button.click();
  await waitFor('the chat transcript', () => document.querySelector('[role="dialog"] .message-row'));
  return { rows: document.querySelectorAll('[role="dialog"] .message-row').length };
`;

export const closeChat = () => `
  document.querySelector('[role="dialog"] button[aria-label="Close"]')?.click();
  return true;
`;

/**
 * Make the page see its window as hidden or shown. The app listens for
 * \`visibilitychange\` and reads \`document.hidden\`; the engine's own
 * treatment of a hidden page (timer throttling, no frames) is not simulated.
 */
export const setHidden = (hidden) => `
  Object.defineProperty(document, 'hidden', { configurable: true, get: () => ${hidden} });
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => '${hidden ? 'hidden' : 'visible'}' });
  document.dispatchEvent(new Event('visibilitychange'));
  return document.hidden;
`;

/**
 * What the page holds for each of \`sessions\` (\`[{ tag, branchId }]\`), without
 * scrolling: whether its card exists, whether its row is mounted, and the
 * newest stamp in the row's subtitle. Returns
 * \`{ at, rows: [{ tag, card, row, kind, seq, sent }] }\`: the page's clock
 * when it looked, and for each session the last three \`null\` when the row
 * shows no stamp.
 */
export const rows = (sessions) => `${HELPERS}
  const at = Date.now();
  const rows = ${JSON.stringify(sessions.map(({ tag, branchId }) => ({ tag, branchId })))}.map(({ tag, branchId }) => {
    const card = cardOf(branchId);
    const row = card && rowIn(card, tag);
    let newest = null;
    for (const item of row ? row.querySelectorAll('.timeline-meta .meta-item') : []) {
      STAMP.lastIndex = 0;
      for (let m = STAMP.exec(item.textContent); m; m = STAMP.exec(item.textContent)) {
        if (m[2] === tag && (!newest || Number(m[3]) > newest.seq))
          newest = { kind: m[1], seq: Number(m[3]), sent: Number(m[4]) };
      }
    }
    return { tag, card: !!card, row: !!row, kind: newest?.kind ?? null, seq: newest?.seq ?? null, sent: newest?.sent ?? null };
  });
  return { at, rows };
`;

/** Every text stamp of session \`tag\` in the open chat, as \`[[seq, sent], ...]\`, or \`null\` with no chat open. */
export const chatStamps = (tag) => `
  const dialog = document.querySelector('[role="dialog"]');
  if (!dialog) return null;
  const out = [];
  const seen = new Set();
  for (const m of dialog.textContent.matchAll(/pfm-x([a-z0-9]+)-(\\d+)-(\\d{13})/g)) {
    if (m[1] !== ${JSON.stringify(tag)} || seen.has(m[2])) continue;
    seen.add(m[2]);
    out.push([Number(m[2]), Number(m[3])]);
  }
  return out;
`;

/** What the page shows, for a result's sanity section. */
export const describe = () => `
  return {
    subtitles: [...document.querySelectorAll('.timeline-meta .meta-item')].filter((el) => /pfm-/.test(el.textContent)).length,
    timelineRows: document.querySelectorAll('.timeline-row').length,
    chatRows: document.querySelectorAll('[role="dialog"] .message-row').length,
    domNodes: document.getElementsByTagName('*').length,
    viewport: { width: window.innerWidth, height: window.innerHeight },
  };
`;
