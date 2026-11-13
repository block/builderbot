import { describe, expect, it } from 'vitest';
import { LatestWins } from './latestWins';

describe('LatestWins', () => {
  it('admits responses that land in the order they were sent', () => {
    const order = new LatestWins();
    const first = order.issue();
    const second = order.issue();

    expect(order.admit('p1', first)).toBe(true);
    expect(order.admit('p1', second)).toBe(true);
  });

  it('refuses a response that lands after a later-sent one', () => {
    const order = new LatestWins();
    const first = order.issue();
    const second = order.issue();

    expect(order.admit('p1', second)).toBe(true);
    expect(order.admit('p1', first)).toBe(false);
    // The refused response must not lower the mark it lost to.
    const third = order.issue();
    expect(order.admit('p1', third)).toBe(true);
    expect(order.admit('p1', second)).toBe(false);
  });

  it('orders each key on its own', () => {
    const order = new LatestWins();
    const first = order.issue();
    const second = order.issue();

    expect(order.admit('p2', second)).toBe(true);
    expect(order.admit('p1', first)).toBe(true);
  });

  it('compares tickets across keys', () => {
    const order = new LatestWins<'list' | 'row'>();
    const list = order.issue();
    const row = order.issue();

    expect(order.overtaken('row', list)).toBe(false);
    expect(order.admit('row', row)).toBe(true);
    expect(order.overtaken('row', list)).toBe(true);
    // `overtaken` only asks; the list response can still be recorded.
    expect(order.admit('list', list)).toBe(true);
  });

  it('knows the newest request sent, whatever has landed', () => {
    const order = new LatestWins();
    const first = order.issue();
    expect(order.isNewest(first)).toBe(true);

    const second = order.issue();
    expect(order.isNewest(first)).toBe(false);
    expect(order.isNewest(second)).toBe(true);
    order.admit('p1', first);
    expect(order.isNewest(second)).toBe(true);
  });

  it('lets a local write mark the reads sent before it landed', () => {
    const order = new LatestWins<'list' | 'written'>();
    const before = order.issue();
    order.admit('written', order.issue());
    const after = order.issue();

    expect(order.overtaken('written', before)).toBe(true);
    expect(order.overtaken('written', after)).toBe(false);
    // The read is still the newest list response to land.
    expect(order.admit('list', before)).toBe(true);
  });
});
