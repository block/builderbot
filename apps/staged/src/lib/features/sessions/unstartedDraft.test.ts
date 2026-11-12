import { describe, expect, it } from 'vitest';
import {
  offerUnstartedDraftBox,
  otherUnstartedDraftBox,
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

  it('does not double the full stop of a reason that ends in one', () => {
    expect(unstartedDraftToast({ reason: 'Agent not found.', keptNewerText: false })).toEqual({
      title: 'The new session did not start',
      description: "Agent not found. Your message is back in the note's chat box.",
    });
    expect(unstartedDraftToast({ reason: 'Not allowed! ', keptNewerText: false }).description).toBe(
      "Not allowed! Your message is back in the note's chat box."
    );
    expect(unstartedDraftMessage({ reason: 'Agent not found.', keptNewerText: false })).toBe(
      'The new session did not start (Agent not found). Your message is back in the box.'
    );
  });

  it('gives no reason when it is only blank space or a full stop', () => {
    expect(unstartedDraftToast({ reason: ' . ', keptNewerText: false }).description).toBe(
      "Your message is back in the note's chat box."
    );
    expect(unstartedDraftMessage({ reason: '', keptNewerText: false })).toBe(
      'The new session did not start. Your message is back in the box.'
    );
  });

  it('claims no box when the pane that held the message is gone', () => {
    expect(unstartedDraftToast({ keptNewerText: false, boxGone: true }).description).toBe(
      "Your message was saved as the note's chat draft. Copy it from here if the chat does not show it."
    );
    expect(
      unstartedDraftToast({ reason: 'boom', keptNewerText: true, boxGone: true }).description
    ).toBe(
      "boom. Your message was saved as the note's chat draft, above what you typed since. Copy it from here if the chat does not show it."
    );
  });

  it('says when the attachments went with that pane', () => {
    expect(
      unstartedDraftToast({ keptNewerText: false, boxGone: true, attachmentsLost: true })
        .description
    ).toBe(
      "Your message was saved as the note's chat draft. Copy it from here if the chat does not show it. Its attachments were not kept."
    );
    // A pane that still has its box keeps them with it.
    expect(unstartedDraftToast({ keptNewerText: false, attachmentsLost: true }).description).toBe(
      "Your message is back in the note's chat box."
    );
  });
});

describe('open composer boxes', () => {
  const box = () => async () => {};

  it('finds another box showing the session, never the asking pane or another session', () => {
    const own = box();
    const other = box();
    expect(otherUnstartedDraftBox('s1', own)).toBeNull();

    const withdrawOwn = offerUnstartedDraftBox('s1', own);
    const withdrawElsewhere = offerUnstartedDraftBox('s2', other);
    expect(otherUnstartedDraftBox('s1', own)).toBeNull();

    const withdrawOther = offerUnstartedDraftBox('s1', other);
    expect(otherUnstartedDraftBox('s1', own)).toBe(other);
    expect(otherUnstartedDraftBox('s1', other)).toBe(own);

    withdrawOther();
    expect(otherUnstartedDraftBox('s1', own)).toBeNull();
    withdrawOwn();
    withdrawElsewhere();
    expect(otherUnstartedDraftBox('s2', own)).toBeNull();
  });

  it('keeps a box offered again after an earlier offer was withdrawn', () => {
    const own = box();
    const other = box();
    const withdrawFirst = offerUnstartedDraftBox('s1', other);
    withdrawFirst();
    const withdrawSecond = offerUnstartedDraftBox('s1', other);
    // A stale withdrawal does not take the newer offer with it.
    withdrawFirst();
    expect(otherUnstartedDraftBox('s1', own)).toBe(other);
    withdrawSecond();
    expect(otherUnstartedDraftBox('s1', own)).toBeNull();
  });
});
