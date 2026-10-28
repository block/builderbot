/**
 * Whether a transcript group mounting right now should slide in.
 *
 * Intros are for live updates. They are off until the opening load has
 * rendered (otherwise every group of the first page slides in at once) and
 * suspended around merges that are not live updates — an older page, a
 * reopen's catch-up delta. Those merges can overlap: a reopen within one poll
 * round trip of the close starts a second catch-up while the first is still
 * awaiting, so a saved-and-restored flag could be put back as `false` by
 * whichever finishes last. A depth count cannot: intros resume once every
 * suspension has been released.
 */
export interface IntroGate {
  /** Read by the transition at mount time. */
  readonly animate: boolean;
  /** The opening load has rendered; live updates may animate from here. */
  enable(): void;
  /** Hold intros off until the returned release runs (idempotent). */
  suspend(): () => void;
}

export function createIntroGate(): IntroGate {
  let enabled = false;
  let depth = 0;
  return {
    get animate() {
      return enabled && depth === 0;
    },
    enable() {
      enabled = true;
    },
    suspend() {
      depth += 1;
      let released = false;
      return () => {
        if (released) return;
        released = true;
        depth -= 1;
      };
    },
  };
}
