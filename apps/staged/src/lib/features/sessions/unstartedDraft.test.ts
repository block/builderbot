import { describe, expect, it } from 'vitest';
import {
  restoreUnstartedAttachments,
  restoreUnstartedDraft,
  unstartedDraftMessage,
  unstartedDraftToast,
} from './unstartedDraft';

describe('restoreUnstartedDraft', () => {
  it('puts the message back into an empty composer as it was', () => {
    expect(restoreUnstartedDraft('Keep me', '')).toEqual({ text: 'Keep me', keptNewerText: false });
    expect(restoreUnstartedDraft('Keep me', ' \n ')).toEqual({
      text: 'Keep me',
      keptNewerText: false,
    });
  });

  it('puts the message above text typed since, dropping neither', () => {
    expect(restoreUnstartedDraft('Keep me', 'typed since')).toEqual({
      text: 'Keep me\n\ntyped since',
      keptNewerText: true,
    });
  });

  it('keeps the newer text as typed apart from its leading blank space', () => {
    expect(restoreUnstartedDraft('Keep me', '\n typed since \n').text).toBe(
      'Keep me\n\ntyped since \n'
    );
  });

  it('does not double a message the composer already holds again', () => {
    expect(restoreUnstartedDraft('Keep me', 'Keep me ')).toEqual({
      text: 'Keep me ',
      keptNewerText: false,
    });
  });

  it('still joins when the newer text merely contains the message', () => {
    expect(restoreUnstartedDraft('ok', 'look at this').text).toBe('ok\n\nlook at this');
  });
});

describe('restoreUnstartedAttachments', () => {
  it('puts the returned attachments first and keeps those attached since', () => {
    expect(restoreUnstartedAttachments(['a', 'b'], ['c'])).toEqual(['a', 'b', 'c']);
    expect(restoreUnstartedAttachments([], ['c'])).toEqual(['c']);
    expect(restoreUnstartedAttachments(['a'], [])).toEqual(['a']);
  });

  it('lists an attachment once', () => {
    expect(restoreUnstartedAttachments(['a', 'b'], ['b', 'c'])).toEqual(['a', 'b', 'c']);
  });
});

describe('unstarted draft notices', () => {
  it('says the session did not start and where the message is', () => {
    expect(unstartedDraftMessage({ keptNewerText: false })).toBe(
      'The new session did not start. Your message is back in the box.'
    );
    expect(unstartedDraftMessage({ keptNewerText: true })).toBe(
      'The new session did not start. Your message is back in the box, above what you typed since.'
    );
  });

  it('includes the reason when the host gave one', () => {
    expect(unstartedDraftMessage({ reason: 'boom', keptNewerText: false })).toBe(
      'The new session did not start (boom). Your message is back in the box.'
    );
  });

  it("points a closed dialog's user at the note's chat", () => {
    expect(unstartedDraftToast({ keptNewerText: false })).toEqual({
      title: 'The new session did not start',
      description: "Your message is back in the note's chat box.",
    });
    expect(unstartedDraftToast({ keptNewerText: true }).description).toBe(
      "Your message is back in the note's chat box, above what you typed since."
    );
    expect(unstartedDraftToast({ reason: 'boom', keptNewerText: false }).description).toBe(
      "boom. Your message is back in the note's chat box."
    );
  });
});
