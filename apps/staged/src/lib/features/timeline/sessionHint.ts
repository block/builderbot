import type { MessageRole, PipelineExecution, SessionSummaryRow } from '../../types';
import { parseQuotedMessage } from '../sessions/composerDraft';
import type { DisplayRootInput } from '../sessions/pathDisplayRoots';
import { formatToolDisplay, stripXmlTags } from '../sessions/sessionModalHelpers';
import { isStagedResumePrompt } from './stagedResumePrompts';

// The one-line subtitle of a running session, derived from data handed in.
// Nothing here fetches, polls or keeps state, so whatever holds a session's
// rows can call it: the poller in `liveSessionHints.ts`, which holds whole
// `SessionMessage`s, and the session stream's owner
// (`stores/sessionStream.svelte.ts`), which holds summary rows.

const MAX_HINT_LENGTH = 72;

/**
 * How many of a session's newest visible rows are kept to derive its hint
 * from. The poller trims its transcript to this, and the session stream's
 * owner sends it as the summary window and trims to it.
 */
export const HINT_ROW_WINDOW = 40;

/**
 * One visible transcript row, as far as the hint reads it. A whole
 * `SessionMessage` is one. So is a summary row from the session stream as it
 * arrives: these are the four fields of the backend's `SummaryRow`
 * (`session_summary.rs` in the store), under the names it serializes them to.
 */
export type HintRow = {
  id: number;
  role: MessageRole;
  /**
   * A `SummaryRow` leaves this out for a `tool_result` row and for the
   * opening prompt, the two texts `deriveHint` never reads
   * (`sessionHint.test.ts` holds it to that). Read as empty when absent.
   */
  content?: string;
  /** Attachment ids of a `user` row. Absent when there are none. */
  imageIds?: string[];
};

type Assert<Holds extends true> = Holds;

/**
 * Holds the stream's wire row to `HintRow`. If `SessionSummaryRow`
 * (`types.ts`) stops being assignable to `HintRow`, whichever of the two
 * changed, this alias fails the type check: a field `deriveHint` reads that a
 * summary row does not carry, or one it carries in another shape.
 */
export type SummaryRowIsHintRow = Assert<SessionSummaryRow extends HintRow ? true : false>;

/** A session's ACP title as a row shows it, or nothing when it has none. */
export function normalizeSessionTitle(title: string | null | undefined): string | undefined {
  if (!title) return undefined;
  const cleaned = stripXmlTags(title).replace(/\s+/g, ' ').trim();
  return cleaned || undefined;
}

function normalizeHintText(text: string): string | undefined {
  const cleaned = stripXmlTags(text)
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/<\/?[^>]+>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  if (!cleaned) return undefined;
  return truncateHint(cleaned);
}

function truncateHint(text: string): string {
  if (text.length <= MAX_HINT_LENGTH) return text;
  let end = MAX_HINT_LENGTH - 1;
  // `slice` counts UTF-16 units, and a character outside the BMP (most emoji)
  // is a pair of them. When the last unit kept is the first of a pair, drop
  // it too: by itself it renders as a replacement glyph.
  const lastKept = text.charCodeAt(end - 1);
  if (lastKept >= 0xd800 && lastKept <= 0xdbff) end -= 1;
  return `${text.slice(0, end).trimEnd()}…`;
}

function withTrailingEllipsis(text: string): string {
  if (/[.!?…]$/.test(text)) {
    if (text.endsWith('.')) {
      return `${text.slice(0, -1)}…`;
    }
    return text;
  }
  return `${text}…`;
}

function formatToolCallHint(content: string, displayRoots?: DisplayRootInput): string | undefined {
  const { verb, detail } = formatToolDisplay(content, displayRoots, true);
  const text = detail ? `${verb} ${detail}` : verb;
  return normalizeHintText(withTrailingEllipsis(text));
}

function formatAssistantHint(content: string): string | undefined {
  const stripped = stripXmlTags(content);

  // When the response contains review fenced blocks, the surrounding text is just
  // LLM preamble (e.g. "I now have a complete picture…") — not a useful hint.
  if (/```review-(?:title|comments)/.test(stripped)) return undefined;

  const lines = stripped
    .replace(/```[\s\S]*?```/g, ' ')
    .split(/\r?\n/)
    .map((line) =>
      line
        .trim()
        .replace(/^[-*]\s+/, '')
        .replace(/^\d+\.\s+/, '')
        .replace(/^#+\s+/, '')
    )
    .filter(Boolean);

  for (let i = lines.length - 1; i >= 0; i--) {
    const normalized = normalizeHintText(lines[i]);
    if (normalized) {
      return normalized;
    }
  }

  return undefined;
}

/**
 * A line that opens or closes a fenced code block: the fence and, on an
 * opening line, its language tag. A run of backticks with more backticks after
 * it on the same line is inline code, not a fence, and is left alone.
 */
const FENCE_MARKER_LINE = /^[ \t]*(?:`{3,}[^`\n]*|~{3,}.*)$/gm;

/**
 * Typed text on one line. Unlike `normalizeHintText`, which is for agent
 * output, this keeps what the user wrote: the content of a fenced block stays
 * (only its fence lines go), and `<` and `>` are text, not tags.
 */
function normalizeReplyText(text: string): string {
  return text.replace(FENCE_MARKER_LINE, ' ').replace(/\s+/g, ' ').trim();
}

/** What stands in for a reply that is attachments and no text. */
function attachmentPlaceholder(count: number): string {
  return count === 1 ? 'Attachment' : `${count} attachments`;
}

/**
 * The hint for a `user` row that is not the opening prompt: the text the user
 * typed, or nothing when the row has none to show.
 *
 * Left out of the text, because they are not the user's own words:
 * - the tagged blocks Staged wraps around a prompt (`XML_BLOCK_TAGS`: action
 *   instructions, branch history, launch context, Pikchr grammar);
 * - the quoted passage of a reply to a note selection.
 *
 * Rows that yield nothing, so the hint stays on whatever is older:
 * - a row that is all tagged block and has no attachment, which is how Staged
 *   writes a Pikchr correction or a `write_note` reminder;
 * - one of Staged's resume prompts (`STAGED_RESUME_PROMPTS`): the automatic
 *   resume after a rate-limit or network pause or after a relaunch, and a
 *   Resume button's prompt, which is the same text as the first of those.
 *
 * Every other `user` row shows, typed or not. A message a project agent sends
 * with `continue_repo_session` is stored as a plain row and is a real
 * instruction to the session, so it shows like a reply. A row with no text
 * left but with attachments (`imageIds`, which holds images and PDFs alike)
 * shows a placeholder: the rows Staged writes by itself carry none, so an
 * attachment marks a message somebody sent.
 *
 * A new tag that Staged wraps around a prompt must be added to
 * `XML_BLOCK_TAGS`. Agent output gets a generic strip of anything shaped like
 * a tag (`normalizeHintText`); a reply does not, because `<` and `>` in typed
 * text are text. `stripXmlTags` is therefore the only thing that takes a
 * wrapper out of a reply, and it removes whole, balanced blocks of the listed
 * tags and nothing else. A wrapper with an unlisted tag shows raw in the
 * subtitle, tags and body, and so does whatever is left of a listed one that
 * is unclosed or nested in itself.
 */
function formatUserHint(message: HintRow): string | undefined {
  const typed = parseQuotedMessage(stripXmlTags(message.content ?? ''))
    .filter((part) => part.type === 'text')
    .map((part) => part.text)
    .join('\n');
  const text = normalizeReplyText(typed);
  if (text) return isStagedResumePrompt(text) ? undefined : truncateHint(text);
  const attachments = message.imageIds?.length ?? 0;
  return attachments > 0 ? attachmentPlaceholder(attachments) : undefined;
}

/**
 * The newest message with something to show, whichever side wrote it: a tool
 * call, assistant text, or a reply from the user. `messages` is in id order
 * (the order the rows were written), so the walk from the end is by recency.
 *
 * The prompt that opened the session (`openingPromptId`) is not a reply: the
 * row's title already speaks for it, and a session nobody has replied to
 * keeps its fallback label until the agent says something.
 */
export function deriveHint(
  messages: readonly HintRow[],
  displayRoots: DisplayRootInput,
  openingPromptId: number | null
): string | undefined {
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    let hint: string | undefined;
    if (message.role === 'tool_call') {
      hint = formatToolCallHint(message.content ?? '', displayRoots);
    } else if (message.role === 'assistant') {
      hint = formatAssistantHint(message.content ?? '');
    } else if (message.role === 'user' && message.id !== openingPromptId) {
      hint = formatUserHint(message);
    }
    if (hint) return hint;
  }
  return undefined;
}

/** The step a session's pipeline is on, for a session whose transcript is still empty. */
export function derivePipelineHint(
  pipeline: PipelineExecution | null | undefined
): string | undefined {
  if (!pipeline?.steps.length) return undefined;
  const runningStep = pipeline.steps.find((step) => step.status === 'running');
  const currentStep = pipeline.steps[pipeline.currentStep];
  const nextStep =
    runningStep ?? currentStep ?? pipeline.steps.find((step) => step.status === 'pending');
  if (!nextStep) return undefined;
  if (nextStep.status === 'succeeded' || nextStep.status === 'failed') return undefined;
  return normalizeHintText(withTrailingEllipsis(nextStep.label));
}
