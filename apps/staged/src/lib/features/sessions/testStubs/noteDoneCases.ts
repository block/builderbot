import casesSource from './noteDone.cases.json?raw';

/**
 * The case table shared with the backend's copies of two rules: what counts
 * as a bare "Done" (`isNoteDoneText`, and `is_note_done_text` in
 * `src-tauri/src/note_done/text.rs`) and which tool name is `write_note`
 * (`isWriteNoteToolName`, and `is_write_note_tool_name` in
 * `src-tauri/src/note_done.rs`). The chat pane hides a closing "Done" by the
 * first pair's TypeScript half, and the backend logs a turn that did not end
 * in one by the Rust half. Both test suites run every row, so the two cannot
 * come to disagree without a failing test. Test-only: nothing at runtime
 * imports this.
 */
export type SharedCase = readonly [label: string, input: string];

interface SharedCases {
  accept: SharedCase[];
  reject: SharedCase[];
}

function readCases(rule: string): SharedCases {
  const table = JSON.parse(casesSource) as Record<string, unknown>;
  const cases = table[rule] as Partial<SharedCases> | undefined;
  for (const verdict of ['accept', 'reject'] as const) {
    const rows = cases?.[verdict];
    if (!Array.isArray(rows) || rows.length === 0) {
      throw new Error(`noteDone.cases.json has no ${rule}.${verdict} cases`);
    }
    for (const row of rows) {
      if (!Array.isArray(row) || row.length !== 2 || row.some((cell) => typeof cell !== 'string')) {
        throw new Error(`${rule}.${verdict}: ${JSON.stringify(row)} is not [label, input]`);
      }
    }
  }
  return cases as SharedCases;
}

export const doneTextCases = readCases('doneText');
export const writeNoteToolNameCases = readCases('writeNoteToolName');
