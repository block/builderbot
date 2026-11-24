import { parse } from 'svelte/compiler';
import { describe, expect, it } from 'vitest';

// NO_SAFARI_AUTOFILL only works as a pair (see safariAutofill.ts), and the
// types cannot stop a call site from writing its own `name` or
// `autocomplete` beside the spread, where the later one wins silently. Most
// of the inputs that carry it have no component test either. So this reads
// the source of every call site: the tag with the spread has no `name` or
// `autocomplete` of its own, and the set of call sites is the one below.
//
// The sources are read with Svelte's own parser, and the checks look at the
// attribute nodes it returns. A scan of the text gets tags wrong where the
// parser does not: a brace inside a string, a comment inside a tag.

const sources = import.meta.glob('/src/**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/**
 * Every file that mentions the constant, with the number of inputs in it
 * that spread it. An input that loses its spread, or a new one that gains
 * it, fails the test until this list says the same. Each entry above 0 was
 * added because Safari offered AutoFill on that input, so do not remove or
 * lower one without checking the input on a device.
 *
 * A file that names the constant without spreading it, in a comment say,
 * is listed with 0. That is deliberate: a mention with no spread is also
 * what a spread that went missing looks like.
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

const CONSTANT = 'NO_SAFARI_AUTOFILL';
const SPREAD = `{...${CONSTANT}}`;
/** What the constant supplies, and so what a call site must not. */
const SUPPLIED = ['name', 'autocomplete'];

/** A node of the parsed file: the template's, or the script's (ESTree). */
type Node = { type: string; start?: number; [key: string]: unknown };

const isNode = (value: unknown): value is Node =>
  typeof value === 'object' && value !== null && typeof (value as Node).type === 'string';

const isConstant = (value: unknown): value is Node =>
  isNode(value) && value.type === 'Identifier' && value.name === CONSTANT;

/** Calls `visit` on every node under `value`, once each. */
function walk(value: unknown, visit: (node: Node) => void, seen = new Set<unknown>()): void {
  if (typeof value !== 'object' || value === null || seen.has(value)) return;
  seen.add(value);
  if (isNode(value)) visit(value);
  for (const child of Object.values(value)) walk(child, visit, seen);
}

type Finding = { line: number; problem: string; fix: string };

type Inspection = {
  /** The number of tags that carry `{...NO_SAFARI_AUTOFILL}`. */
  carriers: number;
  findings: Finding[];
};

/**
 * The tags in a Svelte file that spread the constant, and what is wrong
 * with them. Only code counts: a mention in a comment or a string is not a
 * node, so it is neither a carrier nor a finding.
 */
function inspect(source: string): Inspection {
  const findings: Finding[] = [];
  const add = (node: Node, problem: string, fix: string) =>
    findings.push({ line: source.slice(0, node.start ?? 0).split('\n').length, problem, fix });

  let root: unknown;
  try {
    root = parse(source, { modern: true });
  } catch (error) {
    add(
      { type: 'Root', start: (error as { position?: [number, number] }).position?.[0] },
      `Svelte cannot parse the file (${(error as Error).message.split('\n')[0]})`,
      'fix the markup, its tags could not be checked'
    );
    return { carriers: 0, findings };
  }

  let carriers = 0;
  /** Every reference to the constant, wherever it is. */
  const uses: Node[] = [];
  /** The references that are fine: `{...NO_SAFARI_AUTOFILL}` on a tag, and the import. */
  const accounted = new Set<Node>();

  walk(root, (node) => {
    if (isConstant(node)) uses.push(node);

    const { imported, local } = node;
    if (
      node.type === 'ImportSpecifier' &&
      isNode(imported) &&
      isNode(local) &&
      (isConstant(imported) || isConstant(local))
    ) {
      // The import is not a use. Under another name it gets its own finding.
      accounted.add(imported).add(local);
      if (imported.name !== local.name) {
        add(
          node,
          `the constant is imported as ${String(local.name)}`,
          `import it under its own name, a spread of an alias is not checked`
        );
      }
    }

    // Elements, components and svelte: tags all keep their attributes here.
    if (!Array.isArray(node.attributes)) return;
    const attributes = node.attributes as Node[];
    const own = attributes.filter(
      (attribute) => attribute.type === 'SpreadAttribute' && isConstant(attribute.expression)
    );
    if (own.length === 0) return;
    carriers++;
    for (const spread of own) accounted.add(spread.expression as Node);

    const tag = `<${String(node.name)}>`;
    if (String(node.name).toLowerCase() !== 'input') {
      add(node, `${tag} is not an input`, `spread the constant on an <input> or <Input> only`);
    }
    if (own.length > 1) {
      add(own[1], `${tag} spreads the constant ${own.length} times`, 'keep one');
    }
    for (const attribute of attributes) {
      // An attribute in any form: name="x", name={x} or the {name} shorthand.
      // A bind: is one too. class:name and the other directives are not.
      const supplied =
        (attribute.type === 'Attribute' || attribute.type === 'BindDirective') &&
        SUPPLIED.includes(String(attribute.name).toLowerCase());
      if (supplied) {
        const written = `${attribute.type === 'BindDirective' ? 'bind:' : ''}${String(attribute.name)}`;
        add(
          attribute,
          `${tag} sets its own ${written}`,
          `remove it, the ${String(attribute.name).toLowerCase()} has to be the constant's ` +
            `or Safari offers AutoFill again`
        );
      } else if (attribute.type === 'SpreadAttribute' && !own.includes(attribute)) {
        const expression = attribute.expression as { start: number; end: number };
        add(
          attribute,
          `${tag} also spreads {...${source.slice(expression.start, expression.end)}}`,
          `pass those props one by one, a second spread can carry a name or an autocomplete ` +
            `that is not seen here`
        );
      }
    }
  });

  for (const use of uses) {
    if (accounted.has(use)) continue;
    add(
      use,
      `${CONSTANT} is used, but not as ${SPREAD} on a tag`,
      `write ${SPREAD} on the input itself. An alias, a wrapper object, a condition or a ` +
        `use in the script hides what reaches the input`
    );
  }

  findings.sort((a, b) => a.line - b.line);
  return { carriers, findings };
}

/** Just the problems, for the cases below. */
const problems = (source: string) => inspect(source).findings.map((finding) => finding.problem);

/** The findings of every file that mentions the constant, one line each. */
const violations = (files: Record<string, string>) =>
  Object.entries(files)
    .filter(([, source]) => source.includes(CONSTANT))
    .flatMap(([file, source]) =>
      inspect(source).findings.map(
        (finding) => `${file}:${finding.line} ${finding.problem}: ${finding.fix}`
      )
    );

/**
 * Where `files` and the `listed` call sites disagree, one line each, with
 * what to do about it. A file is looked at when it mentions the constant
 * anywhere, a comment included, or when it is listed.
 */
function mismatches(files: Record<string, string>, listed: Record<string, number>): string[] {
  const found: string[] = [];
  const names = new Set([
    ...Object.keys(listed),
    ...Object.keys(files).filter((file) => files[file].includes(CONSTANT)),
  ]);
  for (const file of [...names].sort()) {
    const expected = listed[file];
    if (!(file in files)) {
      found.push(
        `${file}: listed in CALL_SITES, but there is no such file. If the component moved ` +
          `or was renamed, list its new path.`
      );
      continue;
    }
    const mentioned = files[file].includes(CONSTANT);
    const actual = mentioned ? inspect(files[file]).carriers : 0;
    if (expected === undefined && actual > 0) {
      found.push(
        `${file}: ${actual} tag(s) with ${SPREAD}, not listed. Add it to CALL_SITES with ${actual}.`
      );
    } else if (expected === undefined) {
      found.push(
        `${file}: mentions ${CONSTANT} without spreading it on a tag. If that is intended ` +
          `(a comment, say), add it to CALL_SITES with 0. If an input was meant to carry ` +
          `it, write ${SPREAD} on the input's tag.`
      );
    } else if (actual < expected) {
      found.push(
        `${file}: ${actual} tag(s) with ${SPREAD}, CALL_SITES says ${expected}. Put the ` +
          `spread back on the input that lost it: Safari offers AutoFill on it without. ` +
          `Lower the number only after checking that input on a device.`
      );
    } else if (actual > expected) {
      found.push(
        `${file}: ${actual} tag(s) with ${SPREAD}, CALL_SITES says ${expected}. Set it to ` +
          `${actual} if the new input is meant to carry the constant.`
      );
    } else if (!mentioned) {
      found.push(
        `${file}: listed with 0, but it no longer mentions ${CONSTANT}. Remove it from CALL_SITES.`
      );
    }
  }
  return found;
}

describe('NO_SAFARI_AUTOFILL call sites', () => {
  it('are the inputs Safari offered AutoFill on, and no others', () => {
    // A silently empty glob would pass both of these with nothing checked.
    expect(Object.keys(sources).length).toBeGreaterThan(100);

    expect(
      mismatches(sources, CALL_SITES),
      'CALL_SITES in safariAutofill.guard.test.ts and the .svelte sources disagree. Each ' +
        'line says what to do.'
    ).toEqual([]);
  });

  it('set no name or autocomplete of their own', () => {
    expect(
      violations(sources),
      `An input with ${SPREAD} takes its name and autocomplete from the constant only: ` +
        'Safari drops the exemption when either is changed. Each line says what to do.'
    ).toEqual([]);
  });

  it('reads a tag the way Svelte does', () => {
    // Words that are not attributes: inside a value, a handler, a class:
    // directive, or a comment in the tag.
    expect(
      problems(
        `<Input
           id={nameId}
           // sets name later
           /* autocomplete="on" */
           {...NO_SAFARI_AUTOFILL}
           class="name"
           class:name={editing}
           aria-label="Project name"
           placeholder={'autocomplete="on"'}
           bind:value={draft.name}
           onkeydown={(e) => { if (e.key === 'Enter') save({ name }); }}
           {disabled}
         />`
      )
    ).toEqual([]);

    // A brace in a string does not hide the attributes after it.
    expect(
      problems(
        `<input {...NO_SAFARI_AUTOFILL} onkeydown={(e) => e.key === '{' && open()} name="x" />`
      )
    ).toEqual(['<input> sets its own name']);
    expect(
      problems(
        '<input {...NO_SAFARI_AUTOFILL} placeholder={`{`} data-open={"{"} autocomplete="on" />'
      )
    ).toEqual(['<input> sets its own autocomplete']);
  });

  it('flags what would undo the constant, and nothing else', () => {
    expect(problems('<Input id={nameId} {...NO_SAFARI_AUTOFILL} bind:value={name} />')).toEqual([]);
    expect(problems('<input {...NO_SAFARI_AUTOFILL} aria-label="Project name" />')).toEqual([]);
    // Not a use: no code refers to it.
    expect(problems('<!-- It carries NO_SAFARI_AUTOFILL for the row. -->')).toEqual([]);
    expect(problems('<script>// No NO_SAFARI_AUTOFILL here.</script>')).toEqual([]);
    expect(
      problems(
        `<script>import { NO_SAFARI_AUTOFILL } from '$lib/shared/safariAutofill';</script>
         <Input {...NO_SAFARI_AUTOFILL} />`
      )
    ).toEqual([]);

    expect(problems('<Input {...NO_SAFARI_AUTOFILL} autocomplete="on" />')).toEqual([
      '<Input> sets its own autocomplete',
    ]);
    expect(problems('<input autocomplete={mode} {...NO_SAFARI_AUTOFILL} />')).toEqual([
      '<input> sets its own autocomplete',
    ]);
    expect(problems('<input {...NO_SAFARI_AUTOFILL} name="title" />')).toEqual([
      '<input> sets its own name',
    ]);
    expect(problems('<input NAME="title" {...NO_SAFARI_AUTOFILL} />')).toEqual([
      '<input> sets its own NAME',
    ]);
    expect(problems('<Input {...NO_SAFARI_AUTOFILL} {name} />')).toEqual([
      '<Input> sets its own name',
    ]);
    expect(problems('<Input {...NO_SAFARI_AUTOFILL} bind:name />')).toEqual([
      '<Input> sets its own bind:name',
    ]);
    expect(problems('<Input {...NO_SAFARI_AUTOFILL} {...rest} />')).toEqual([
      '<Input> also spreads {...rest}',
    ]);
    expect(problems('<Input {...NO_SAFARI_AUTOFILL} {...NO_SAFARI_AUTOFILL} />')).toEqual([
      '<Input> spreads the constant 2 times',
    ]);
    expect(problems('<textarea {...NO_SAFARI_AUTOFILL}></textarea>')).toEqual([
      '<textarea> is not an input',
    ]);

    // Anything but a plain spread on a tag hides what reaches the input.
    const stray = 'NO_SAFARI_AUTOFILL is used, but not as {...NO_SAFARI_AUTOFILL} on a tag';
    expect(problems('<Input {...{ ...NO_SAFARI_AUTOFILL, name: "title" }} />')).toEqual([stray]);
    expect(problems('<Input {...(plain ? {} : NO_SAFARI_AUTOFILL)} />')).toEqual([stray]);
    expect(problems('<Input name={NO_SAFARI_AUTOFILL.name} />')).toEqual([stray]);
    expect(
      problems('<script>const attrs = NO_SAFARI_AUTOFILL;</script><Input {...attrs} />')
    ).toEqual([stray]);
    expect(
      problems(
        `<script>import { NO_SAFARI_AUTOFILL as attrs } from '$lib/shared/safariAutofill';</script>
         <Input {...attrs} />`
      )
    ).toEqual(['the constant is imported as attrs']);

    // A file Svelte rejects is reported, not skipped.
    expect(problems('<input {...NO_SAFARI_AUTOFILL} name="a" name="b" />')).toEqual([
      expect.stringMatching(/^Svelte cannot parse the file \(.+\)$/),
    ]);
  });

  it('says where a problem is and what to do about it', () => {
    expect(
      violations({
        '/src/Clean.svelte': '<Input {...NO_SAFARI_AUTOFILL} />',
        '/src/Unrelated.svelte': '<Input name="title" />',
        '/src/Own.svelte':
          '<form>\n  <Input\n    {...NO_SAFARI_AUTOFILL}\n    {name}\n  />\n</form>',
      })
    ).toEqual([
      "/src/Own.svelte:4 <Input> sets its own name: remove it, the name has to be the constant's " +
        'or Safari offers AutoFill again',
    ]);
  });

  it('says what to do when the list and the sources disagree', () => {
    const one = '<Input {...NO_SAFARI_AUTOFILL} />';
    const two = `${one}<input {...NO_SAFARI_AUTOFILL} />`;
    const comment = '<!-- No NO_SAFARI_AUTOFILL here on purpose. --><Input />';
    const none = '<Input />';

    expect(
      mismatches(
        { '/a.svelte': one, '/b.svelte': two, '/c.svelte': comment, '/d.svelte': none },
        { '/a.svelte': 1, '/b.svelte': 2, '/c.svelte': 0 }
      )
    ).toEqual([]);

    const said = (files: Record<string, string>, listed: Record<string, number>) => {
      const found = mismatches(files, listed);
      expect(found).toHaveLength(1);
      return found[0];
    };
    // A mention with no spread, in a file that is not listed.
    expect(said({ '/c.svelte': comment }, {})).toContain('add it to CALL_SITES with 0');
    // A spread that was dropped, with and without the import left behind.
    expect(said({ '/b.svelte': one }, { '/b.svelte': 2 })).toContain('Put the spread back');
    expect(said({ '/a.svelte': none }, { '/a.svelte': 1 })).toContain('Put the spread back');
    // A new input, in a new file and in a listed one.
    expect(said({ '/a.svelte': one }, {})).toContain('Add it to CALL_SITES with 1');
    expect(said({ '/a.svelte': two }, { '/a.svelte': 1 })).toContain('Set it to 2');
    // Entries that have gone stale.
    expect(said({}, { '/a.svelte': 1 })).toContain('list its new path');
    expect(said({ '/d.svelte': none }, { '/d.svelte': 0 })).toContain('Remove it from CALL_SITES');
  });
});
