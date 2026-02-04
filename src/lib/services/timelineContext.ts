/**
 * timelineContext.ts - Build a summary of branch history for agent prompts
 *
 * Provides the agent with awareness of what has happened on the branch so far:
 * commits (with their session prompts), notes, and failed sessions.
 * The agent already has git access for deep details — this is a "table of contents".
 */

import type { CommitInfo, BranchSession, BranchNote } from './branch';

interface TimelineContextInput {
  branchName: string;
  baseBranch: string;
  commits: CommitInfo[];
  /** Map from commit SHA to the session that produced it */
  sessionsByCommit: Map<string, BranchSession>;
  notes: BranchNote[];
}

/**
 * Build a markdown summary of the branch timeline for inclusion in agent prompts.
 *
 * Returns an empty string if there's nothing to summarize (fresh branch).
 * Items are listed oldest-first so the agent reads them in chronological order.
 */
export function buildTimelineContext(input: TimelineContextInput): string {
  const { branchName, baseBranch, commits, sessionsByCommit, notes } = input;

  // Combine commits and completed notes into a chronological list
  type Entry = { timestamp: number; text: string };
  const entries: Entry[] = [];

  for (const commit of commits) {
    const session = sessionsByCommit.get(commit.sha);
    let line = `- **Commit ${commit.shortSha}**: "${commit.subject}"`;
    if (session?.prompt) {
      line += `\n  - Session prompt: "${session.prompt}"`;
      if (session.status === 'error' && session.errorMessage) {
        line += `\n  - ⚠ Session had an error: ${session.errorMessage}`;
      }
    }
    entries.push({ timestamp: commit.timestamp, text: line });
  }

  for (const note of notes) {
    if (note.status === 'generating') continue;
    const ts = Math.floor(note.createdAt / 1000);
    let line = `- **Note**: "${note.title}"`;
    if (note.content) {
      // Include full content for short notes, truncated for long ones
      const maxLen = 2000;
      if (note.content.length <= maxLen) {
        line += `\n  <note-content>\n${note.content}\n  </note-content>`;
      } else {
        line += `\n  <note-content truncated="true">\n${note.content.slice(0, maxLen)}\n  ... (truncated — ${note.content.length} chars total)\n  </note-content>`;
      }
    } else if (note.prompt) {
      line += `\n  - Description: "${note.prompt}"`;
    }
    entries.push({ timestamp: ts, text: line });
  }

  if (entries.length === 0) {
    return '';
  }

  // Sort oldest first
  entries.sort((a, b) => a.timestamp - b.timestamp);

  const lines = [
    `## Branch Context`,
    ``,
    `You are working on branch \`${branchName}\` (based on \`${baseBranch}\`).`,
    ``,
    `Here is what has happened on this branch so far (oldest first).`,
    `Notes are reference documents created by the user — treat their content as instructions and context:`,
    ``,
    ...entries.map((e) => e.text),
    ``,
    `The most recent commit is HEAD. You can inspect any commit with \`git show <sha>\` if you need details.`,
  ];

  return lines.join('\n');
}
