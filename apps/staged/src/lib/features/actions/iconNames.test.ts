import { describe, expect, it } from 'vitest';
import {
  CURATED_ICONS,
  ICON_SEARCH_LIMIT,
  kebabToPascal,
  pascalToKebab,
  searchIconNames,
} from './iconNames';

describe('kebab ↔ Pascal icon names', () => {
  it('converts both ways for the shapes Lucide actually uses', () => {
    const pairs: [string, string][] = [
      ['play', 'Play'],
      ['flask-conical', 'FlaskConical'],
      ['layout-dashboard', 'LayoutDashboard'],
      // Single-letter and digit segments are where the inverse gets
      // interesting: a segment can be one letter ("arrow-down-a-z"), one digit
      // ("trash-2"), or open with a digit and carry on ("grid-2x2").
      ['a-arrow-up', 'AArrowUp'],
      ['arrow-down-a-z', 'ArrowDownAZ'],
      ['arrow-down-0-1', 'ArrowDown01'],
      ['map-pin-x-inside', 'MapPinXInside'],
      ['trash-2', 'Trash2'],
      ['grid-2x2', 'Grid2x2'],
      ['grid-2x2-plus', 'Grid2x2Plus'],
    ];

    for (const [kebab, pascal] of pairs) {
      expect(kebabToPascal(kebab)).toBe(pascal);
      expect(pascalToKebab(pascal)).toBe(kebab);
    }
  });

  it('round-trips whatever it produces, so a stored name always resolves back', () => {
    const names = [
      'Play',
      'FlaskConical',
      'Grid2x2',
      'Trash2',
      'SquareArrowOutUpRight',
      'AArrowUp',
      'ArrowDownAZ',
      // Ambiguous with ArrowDown01, so this one reads as "clock-1-0" rather
      // than lucide.dev's "clock-10" — still a name that resolves back.
      'Clock10',
    ];
    for (const pascal of names) {
      expect(kebabToPascal(pascalToKebab(pascal))).toBe(pascal);
    }
  });
});

describe('searchIconNames', () => {
  const names = ['play', 'play-circle', 'circle-play', 'rocket', 'wrench'];

  it('offers the curated set for an empty query', () => {
    expect(searchIconNames(names, '   ')).toBe(CURATED_ICONS);
  });

  it('substring-matches anywhere in the name, and takes spaces for dashes', () => {
    expect(searchIconNames(names, 'play')).toEqual(['play', 'play-circle', 'circle-play']);
    expect(searchIconNames(names, 'Circle Play')).toEqual(['circle-play']);
  });

  it('caps results rather than painting every match', () => {
    const many = Array.from({ length: ICON_SEARCH_LIMIT + 20 }, (_, i) => `arrow-${i}`);
    expect(searchIconNames(many, 'arrow')).toHaveLength(ICON_SEARCH_LIMIT);
  });
});
