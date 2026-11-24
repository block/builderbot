// What the driver does in the app's UI, as scripts run inside the page: the
// one place that knows the app's markup. A tree whose markup differs needs
// only these selectors changed.
//
// Each export returns the body of an async function for `frontend.evaluate`.

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
  const rowOf = (tag) => [...document.querySelectorAll('.timeline-row')].find((row) => visible(row) && row.textContent.includes('pf-tag=' + tag));
`;

/** Open a project's detail from wherever the page is, and wait for a session's row. */
export const openProject = (projectName, tag) => `${HELPERS}
  if (!rowOf(${JSON.stringify(tag)})) {
    const name = await waitFor('the project ${projectName}', () =>
      [...document.querySelectorAll('*')].find((el) => el.children.length === 0 && visible(el) && el.textContent.trim() === ${JSON.stringify(projectName)}));
    name.click();
  }
  await waitFor('the row of session ${tag}', () => rowOf(${JSON.stringify(tag)}));
  return { rows: document.querySelectorAll('.timeline-row').length };
`;

/** Open a session's chat from its row, and wait for its transcript. */
export const openChat = (tag) => `${HELPERS}
  const row = await waitFor('the row of session ${tag}', () => rowOf(${JSON.stringify(tag)}));
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

/** What the page shows, for a result's sanity section. */
export const describe = () => `
  return {
    subtitles: [...document.querySelectorAll('.timeline-meta .meta-item')].filter((el) => /pfm-/.test(el.textContent)).length,
    timelineRows: document.querySelectorAll('.timeline-row').length,
    chatRows: document.querySelectorAll('[role="dialog"] .message-row').length,
    domNodes: document.getElementsByTagName('*').length,
  };
`;
