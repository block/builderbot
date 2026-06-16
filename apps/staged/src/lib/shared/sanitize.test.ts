import { describe, expect, it } from 'vitest';
import { sanitize } from './sanitize';

describe('sanitize', () => {
  it('keeps hashtag-badge data attributes on span while stripping style', () => {
    const dirty =
      '<span class="hashtag-badge type-note" data-hashtag-kind="note" ' +
      'data-hashtag-type="note" data-hashtag-id="abc123" ' +
      'data-hashtag-ref="#note:abc123" style="background: red; color: white;">My note</span>';
    const clean = sanitize(dirty);

    expect(clean).toContain('class="hashtag-badge type-note"');
    expect(clean).toContain('data-hashtag-kind="note"');
    expect(clean).toContain('data-hashtag-type="note"');
    expect(clean).toContain('data-hashtag-id="abc123"');
    expect(clean).toContain('data-hashtag-ref="#note:abc123"');
    // style must always be stripped — badge colours come from CSS classes.
    expect(clean).not.toContain('style');
    expect(clean).not.toContain('background');
  });

  it('does not allow data-hashtag attributes on other tags', () => {
    const dirty =
      '<div data-hashtag-kind="note" data-hashtag-type="note" ' +
      'data-hashtag-id="abc123" data-hashtag-ref="#note:abc123">x</div>';
    const clean = sanitize(dirty);

    expect(clean).not.toContain('data-hashtag-kind');
    expect(clean).not.toContain('data-hashtag-type');
    expect(clean).not.toContain('data-hashtag-id');
    expect(clean).not.toContain('data-hashtag-ref');
  });

  it('strips script tags', () => {
    expect(sanitize('<script>alert(1)</script>hi')).not.toContain('<script>');
  });
});
