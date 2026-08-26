import { describe, expect, it } from 'vitest';

import { computeFooterOverflow, overflowedActions } from './footerOverflow';

const ALL = { note: true, commit: true, review: true };

describe('computeFooterOverflow', () => {
  it('keeps every button on a wide card', () => {
    expect(computeFooterOverflow(720, ALL)).toEqual({
      note: false,
      commit: false,
      review: false,
    });
  });

  it('sheds buttons review-first, then commit, then note', () => {
    expect(computeFooterOverflow(370, ALL)).toMatchObject({ review: true, commit: false });
    expect(computeFooterOverflow(310, ALL)).toMatchObject({ review: true, commit: true });
    expect(computeFooterOverflow(250, ALL)).toEqual({ note: true, commit: true, review: true });
  });

  it('reports actions the card is not offering as hidden', () => {
    expect(computeFooterOverflow(720, { note: true, commit: true })).toEqual({
      note: false,
      commit: false,
      review: true,
    });
  });

  it('gives the remaining buttons the space an absent one frees', () => {
    // Two buttons still fit at 340px even though three would not.
    expect(computeFooterOverflow(340, { note: true, commit: true })).toMatchObject({
      note: false,
      commit: false,
    });
    expect(computeFooterOverflow(340, ALL)).toMatchObject({ review: true });
  });

  it('shows everything until the container has been measured', () => {
    expect(computeFooterOverflow(0, ALL)).toEqual({ note: false, commit: false, review: false });
  });
});

describe('overflowedActions', () => {
  it('lists overflowed actions in button order', () => {
    expect(overflowedActions(computeFooterOverflow(250, ALL), ALL)).toEqual([
      'note',
      'commit',
      'review',
    ]);
  });

  it('omits actions the card never offered', () => {
    const available = { note: true, commit: true };

    expect(overflowedActions(computeFooterOverflow(250, available), available)).toEqual([
      'note',
      'commit',
    ]);
  });

  it('is empty while everything fits', () => {
    expect(overflowedActions(computeFooterOverflow(720, ALL), ALL)).toEqual([]);
  });
});
