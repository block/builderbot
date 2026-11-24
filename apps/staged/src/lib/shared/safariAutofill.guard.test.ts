import { describe, expect, it } from 'vitest';

// NO_SAFARI_AUTOFILL only works as a pair (see safariAutofill.ts), and the
// types cannot stop a call site from writing its own `name` or
// `autocomplete` beside the spread, where the later one wins silently. Most
// of the inputs that carry it have no component test either. So this reads
// the source of every call site: the tag with the spread has no `name` or
// `autocomplete` of its own, and the set of call sites is the one below.

const sources = import.meta.glob('/src/**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/**
 * Every file that spreads the constant, with the number of inputs in it
 * that do. An input that loses its spread, or a new one that gains it,
 * fails the test until this list says the same. Each entry was added
 * because Safari offered AutoFill on that input, so do not remove one
 * without checking the input on a device.
 */
const CALL_SITES: Record<string, number> = {
  '/src/lib/features/actions/ActionEditDialog.svelte': 1,
  '/src/lib/features/board/BoardColumn.svelte': 1,
  '/src/lib/features/branches/RenameBranchDialog.svelte': 1,
  '/src/lib/features/doctor/AgentLoginPrompt.svelte': 1,
  '/src/lib/features/projects/NewProjectForm.svelte': 1,
  '/src/lib/features/projects/ProjectSection.svelte': 1,
  // The env var name and its value.
  '/src/lib/features/settings/AgentEnvVarsSetting.svelte': 2,
  '/src/lib/features/settings/ActionsSettingsPanel.svelte': 1,
  '/src/lib/features/settings/GeneralSettingsPanel.svelte': 1,
  '/src/lib/features/settings/tags/TagGroupSection.svelte': 1,
  '/src/lib/features/settings/tags/TagRow.svelte': 1,
  '/src/lib/features/tags/NewTagDialog.svelte': 1,
};

const SPREAD = '...NO_SAFARI_AUTOFILL';

type Tag = {
  name: string;
  /** Attribute names as written, a `{name}` shorthand included. */
  attributes: string[];
  /** Each `{...x}` on the tag, as `...x`. */
  spreads: string[];
};

/** The index just past the `}` that closes the `{` at `open`. */
function closingBrace(source: string, open: number): number {
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    if (source[i] === '{') depth++;
    else if (source[i] === '}' && --depth === 0) return i + 1;
  }
  return source.length;
}

/**
 * Every opening tag in a Svelte file, with its attributes read at the tag's
 * own level: nothing inside a quoted value or a `{…}` expression counts as
 * an attribute. A `<` in the script can start a bogus tag (`Array<string>`),
 * which is harmless here: only tags that hold the spread are looked at, and
 * a spread that no tag holds is reported.
 */
function openingTags(source: string): Tag[] {
  const tags: Tag[] = [];
  for (const match of source.matchAll(/<([A-Za-z][\w.:-]*)(?=[\s/>])/g)) {
    const tag: Tag = { name: match[1], attributes: [], spreads: [] };
    let i = match.index + match[0].length;
    while (i < source.length && source[i] !== '>') {
      const ch = source[i];
      if (/[\s/]/.test(ch)) {
        i++;
      } else if (ch === '{') {
        const end = closingBrace(source, i);
        const inner = source.slice(i + 1, end - 1).trim();
        if (inner.startsWith('...')) tag.spreads.push(inner.replace(/^\.\.\.\s*/, '...'));
        else tag.attributes.push(inner);
        i = end;
      } else {
        const name = source.slice(i).match(/^[^\s=>/]+/)?.[0] ?? ch;
        tag.attributes.push(name);
        i += name.length;
        if (source[i] !== '=') continue;
        i++;
        if (source[i] === '"' || source[i] === "'") {
          const close = source.indexOf(source[i], i + 1);
          i = close === -1 ? source.length : close + 1;
        } else if (source[i] === '{') {
          i = closingBrace(source, i);
        } else {
          i += source.slice(i).match(/^[^\s>]*/)![0].length;
        }
      }
    }
    tags.push(tag);
  }
  return tags;
}

/** What is wrong with each tag in `source` that spreads the constant. */
function problems(source: string): string[] {
  const found: string[] = [];
  const carriers = openingTags(source).filter((tag) => tag.spreads.includes(SPREAD));
  for (const tag of carriers) {
    if (tag.name.toLowerCase() !== 'input') found.push(`<${tag.name}> is not an input`);
    for (const attribute of tag.attributes) {
      if (['name', 'autocomplete'].includes(attribute.toLowerCase())) {
        found.push(`<${tag.name}> sets its own ${attribute}`);
      }
    }
    // Another spread could carry a `name` or `autocomplete` this cannot see.
    for (const spread of tag.spreads) {
      if (spread !== SPREAD) found.push(`<${tag.name}> also spreads {${spread}}`);
    }
  }
  // A mention in a comment has no `...` in front of it, so this counts uses.
  const uses = source.match(/\.\.\.\s*NO_SAFARI_AUTOFILL\b/g)?.length ?? 0;
  const onTags = carriers.reduce(
    (count, tag) => count + tag.spreads.filter((spread) => spread === SPREAD).length,
    0
  );
  if (uses !== onTags) found.push(`${uses - onTags} use(s) that are not a spread on a tag`);
  return found;
}

/** The number of tags in `source` that spread the constant. */
const spreadCount = (source: string) =>
  openingTags(source).filter((tag) => tag.spreads.includes(SPREAD)).length;

describe('NO_SAFARI_AUTOFILL call sites', () => {
  it('are the inputs Safari offered AutoFill on, and no others', () => {
    // A silently empty glob would pass the next test with nothing checked.
    expect(Object.keys(sources).length).toBeGreaterThan(100);

    const actual: Record<string, number> = {};
    for (const [file, source] of Object.entries(sources)) {
      if (source.includes('NO_SAFARI_AUTOFILL')) actual[file] = spreadCount(source);
    }

    expect(
      actual,
      'Each input in CALL_SITES needs {...NO_SAFARI_AUTOFILL}: Safari offers AutoFill on it ' +
        'without. For a new input that takes the spread, add it to CALL_SITES.'
    ).toEqual(CALL_SITES);
  });

  it('set no name or autocomplete of their own', () => {
    const violations = Object.entries(sources).flatMap(([file, source]) =>
      problems(source).map((problem) => `${file}: ${problem}`)
    );

    expect(
      violations,
      'An input with {...NO_SAFARI_AUTOFILL} takes its name and autocomplete from the ' +
        'constant only. Safari drops the exemption when either is changed.'
    ).toEqual([]);
  });

  it('reads attributes at the level of the tag only', () => {
    const [tag] = openingTags(
      `<Input
         id={nameId}
         {...NO_SAFARI_AUTOFILL}
         class="column-name-input"
         aria-label="Project name"
         placeholder={'autocomplete="on"'}
         bind:value={draft.name}
         onkeydown={(e) => { if (e.key === 'Enter') save({ name }); }}
         {disabled}
       />`
    );

    expect(tag).toEqual({
      name: 'Input',
      attributes: [
        'id',
        'class',
        'aria-label',
        'placeholder',
        'bind:value',
        'onkeydown',
        'disabled',
      ],
      spreads: [SPREAD],
    });
  });

  it('flags what would undo the constant, and nothing else', () => {
    expect(problems('<Input id={nameId} {...NO_SAFARI_AUTOFILL} bind:value={name} />')).toEqual([]);
    expect(problems('<input {...NO_SAFARI_AUTOFILL} aria-label="Project name" />')).toEqual([]);
    // Not a use: no spread.
    expect(problems('<!-- It carries NO_SAFARI_AUTOFILL for the row. -->')).toEqual([]);

    expect(problems('<Input {...NO_SAFARI_AUTOFILL} autocomplete="on" />')).toEqual([
      '<Input> sets its own autocomplete',
    ]);
    expect(problems('<input autocomplete={mode} {...NO_SAFARI_AUTOFILL} />')).toEqual([
      '<input> sets its own autocomplete',
    ]);
    expect(problems('<input {...NO_SAFARI_AUTOFILL} name="title" />')).toEqual([
      '<input> sets its own name',
    ]);
    expect(problems('<Input {...NO_SAFARI_AUTOFILL} {name} />')).toEqual([
      '<Input> sets its own name',
    ]);
    expect(problems('<Input {...NO_SAFARI_AUTOFILL} {...rest} />')).toEqual([
      '<Input> also spreads {...rest}',
    ]);
    expect(problems('<textarea {...NO_SAFARI_AUTOFILL}></textarea>')).toEqual([
      '<textarea> is not an input',
    ]);
    // Wrapped in another object, or used in the script, it is not a plain
    // spread on a tag, and what is merged with it cannot be seen.
    expect(problems('<Input {...{ ...NO_SAFARI_AUTOFILL, name: "title" }} />')).toEqual([
      '1 use(s) that are not a spread on a tag',
    ]);
    expect(problems('<script>const attrs = { ...NO_SAFARI_AUTOFILL };</script>')).toEqual([
      '1 use(s) that are not a spread on a tag',
    ]);
  });
});
