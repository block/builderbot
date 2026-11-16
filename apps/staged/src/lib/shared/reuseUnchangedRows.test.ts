import { describe, expect, it } from 'vitest';
import { reuseUnchangedRows, sameRow } from './reuseUnchangedRows';

interface Row {
  id: string;
  name: string;
  position: number;
  note?: string | null;
}

const byId = (row: Row) => row.id;

function rows(): Row[] {
  return [
    { id: 'a', name: 'Alpha', position: 0 },
    { id: 'b', name: 'Beta', position: 1 },
    { id: 'c', name: 'Gamma', position: 2 },
  ];
}

describe('sameRow', () => {
  it('holds for one object and for two with the same fields and values', () => {
    const row = rows()[0];
    expect(sameRow(row, row)).toBe(true);
    expect(sameRow(row, { ...row })).toBe(true);
  });

  it('fails when a value differs', () => {
    expect(sameRow(rows()[0], { ...rows()[0], name: 'Renamed' })).toBe(false);
  });

  it('counts a field only one of the rows carries, whichever one it is', () => {
    const bare = rows()[0];
    const noted = { ...bare, note: null };
    expect(sameRow<Row>(bare, noted)).toBe(false);
    expect(sameRow<Row>(noted, bare)).toBe(false);
    // Present and undefined is still a field the other row lacks a value for.
    expect(sameRow<Row>(bare, { ...bare, note: undefined })).toBe(true);
  });

  it('compares values strictly, without coercion', () => {
    const numbered = { id: 'a', value: 0 as unknown };
    expect(sameRow(numbered, { id: 'a', value: '0' })).toBe(false);
    expect(sameRow(numbered, { id: 'a', value: false })).toBe(false);
    expect(sameRow({ id: 'a', value: null as unknown }, { id: 'a', value: undefined })).toBe(false);
    expect(sameRow({ id: 'a', value: NaN }, { id: 'a', value: NaN })).toBe(true);
  });

  it('compares one level deep: a nested value is the same only by identity', () => {
    const nested = { labels: ['x'] };
    expect(sameRow({ id: 'a', nested }, { id: 'a', nested })).toBe(true);
    expect(sameRow({ id: 'a', nested }, { id: 'a', nested: { labels: ['x'] } })).toBe(false);
  });
});

describe('reuseUnchangedRows', () => {
  it('returns the list held when the fetched one changes nothing', () => {
    const current = rows();
    expect(reuseUnchangedRows(current, rows(), byId)).toBe(current);
  });

  it('returns the list held for two empty lists', () => {
    const current: Row[] = [];
    expect(reuseUnchangedRows(current, [], byId)).toBe(current);
  });

  it('keeps the unchanged rows and takes the fetched object of a changed one', () => {
    const current = rows();
    const fetched = rows();
    fetched[1] = { ...fetched[1], name: 'Renamed' };

    const next = reuseUnchangedRows(current, fetched, byId);

    expect(next).not.toBe(current);
    expect(next).toEqual(fetched);
    expect(next[0]).toBe(current[0]);
    expect(next[1]).toBe(fetched[1]);
    expect(next[2]).toBe(current[2]);
  });

  it('follows the fetched order, keeping every object', () => {
    const current = rows();

    const next = reuseUnchangedRows(current, rows().reverse(), byId);

    expect(next).not.toBe(current);
    expect(next).toEqual(rows().reverse());
    expect(next).toEqual([current[2], current[1], current[0]]);
    expect(next.every((row) => current.includes(row))).toBe(true);
  });

  it('is a new list when a row is added, with the old rows kept', () => {
    const current = rows();
    const added = { id: 'd', name: 'Delta', position: 3 };

    const next = reuseUnchangedRows(current, [...rows(), added], byId);

    expect(next).not.toBe(current);
    expect(next.slice(0, 3).every((row, i) => row === current[i])).toBe(true);
    expect(next[3]).toBe(added);
  });

  it('is a new list when a row is removed, the last one included', () => {
    const current = rows();

    const next = reuseUnchangedRows(current, rows().slice(0, 2), byId);

    expect(next).not.toBe(current);
    expect(next).toEqual([current[0], current[1]]);
    expect(next[0]).toBe(current[0]);
  });

  it('matches rows by the key it is given, not by position', () => {
    const current = rows();
    // The same values under another key: a different row, so not reused.
    const fetched = rows().map((row) => ({ ...row }));
    const byName = (row: Row) => row.name;
    fetched[0] = { ...fetched[0], name: 'Other' };

    const next = reuseUnchangedRows(current, fetched, byName);

    expect(next[0]).toBe(fetched[0]);
    expect(next[1]).toBe(current[1]);
  });

  it('never changes the list held or the fetched one', () => {
    const current = rows();
    const fetched = rows();
    fetched[2] = { ...fetched[2], position: 9 };
    const currentBefore = [...current];
    const fetchedBefore = [...fetched];

    reuseUnchangedRows(current, fetched, byId);

    expect(current).toEqual(currentBefore);
    expect(current.every((row, i) => row === currentBefore[i])).toBe(true);
    expect(fetched.every((row, i) => row === fetchedBefore[i])).toBe(true);
  });
});
