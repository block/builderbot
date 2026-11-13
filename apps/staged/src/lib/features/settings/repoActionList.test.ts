import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectAction } from '../../api/commands';
import type { RepoActionScope } from './repoActionList.svelte';

function action(id: string): ProjectAction {
  return {
    id,
    name: id,
    command: `just ${id}`,
    actionType: 'run',
    sortOrder: 1,
    pinned: false,
    icon: null,
  } as unknown as ProjectAction;
}

describe('RepoActionList', () => {
  /** The reads sent, in order, each answered by hand. */
  let reads: Array<{
    githubRepo: string;
    answer: (actions: ProjectAction[]) => void;
    fail: (reason: Error) => void;
  }>;
  let selected: RepoActionScope | null;

  beforeEach(() => {
    vi.resetModules();
    // A .svelte.ts module compiled without the Svelte plugin here, so the
    // rune call resolves to this pass-through global.
    vi.stubGlobal('$state', (initial: unknown) => initial);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    reads = [];
    selected = { githubRepo: 'acme/app' };
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  async function newList() {
    const { RepoActionList } = await import('./repoActionList.svelte');
    return new RepoActionList(
      () => selected,
      (githubRepo) =>
        new Promise<ProjectAction[]>((answer, fail) => reads.push({ githubRepo, answer, fail }))
    );
  }

  const ids = (actions: ProjectAction[]) => actions.map((a) => a.id);

  it('lists what the read returns', async () => {
    const list = await newList();
    const load = list.load();
    expect(list.loading).toBe(true);

    reads[0].answer([action('build'), action('test')]);
    await load;
    expect(ids(list.actions)).toEqual(['build', 'test']);
    expect(list.loading).toBe(false);
  });

  it('keeps the read sent last when an earlier one answers after it', async () => {
    const list = await newList();
    const first = list.load();
    selected = { githubRepo: 'acme/web' };
    const second = list.load();

    reads[1].answer([action('web-dev')]);
    await second;
    reads[0].answer([action('app-dev')]);
    await first;
    expect(ids(list.actions)).toEqual(['web-dev']);
  });

  it('does not put a deleted action back when a read sent before the delete lands after it', async () => {
    const list = await newList();
    const initial = list.load();
    reads[0].answer([action('build'), action('test')]);
    await initial;

    // A reload is in flight and read the store before the delete.
    const reload = list.load();
    // The delete lands: the panel drops the row.
    list.write(list.actions.filter((a) => a.id !== 'test'));
    reads[1].answer([action('build'), action('test')]);
    await vi.waitFor(() => expect(reads).toHaveLength(3));
    expect(ids(list.actions)).toEqual(['build']);

    // The list is read again, now with the delete in the store, rather than
    // dropped: this read brings an action the first two did not have.
    reads[2].answer([action('build'), action('lint')]);
    await reload;
    expect(ids(list.actions)).toEqual(['build', 'lint']);
    expect(list.loading).toBe(false);
  });

  it('reads the repo selected by then when a write lands under an earlier read', async () => {
    const list = await newList();
    const initial = list.load();
    reads[0].answer([action('app-dev'), action('app-test')]);
    await initial;

    // Another repo is picked while the first one's rows are still listed,
    // and a delete of one of those rows lands before its read does.
    selected = { githubRepo: 'acme/web' };
    const switched = list.load();
    list.write(list.actions.filter((a) => a.id !== 'app-test'));
    reads[1].answer([action('web-dev')]);
    await vi.waitFor(() => expect(reads).toHaveLength(3));

    expect(reads[2].githubRepo).toBe('acme/web');
    reads[2].answer([action('web-dev')]);
    await switched;
    expect(ids(list.actions)).toEqual(['web-dev']);
  });

  it('applies a read sent after the write landed', async () => {
    const list = await newList();
    list.write([action('detected')]);
    const load = list.load();
    reads[0].answer([action('detected'), action('added-elsewhere')]);
    await load;

    expect(reads).toHaveLength(1);
    expect(ids(list.actions)).toEqual(['detected', 'added-elsewhere']);
  });

  it('empties the list when the read fails, and when no repo is selected', async () => {
    const list = await newList();
    list.write([action('build')]);
    const failed = list.load();
    reads[0].fail(new Error('Database not initialized'));
    await failed;
    expect(list.actions).toEqual([]);
    expect(list.loading).toBe(false);

    list.write([action('build')]);
    const inFlight = list.load();
    selected = null;
    await list.load();
    expect(list.actions).toEqual([]);
    // The read still out must not fill a list with no repo selected.
    reads[1].answer([action('build')]);
    await inFlight;
    expect(list.actions).toEqual([]);
  });
});
