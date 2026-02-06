---
name: monitor-reviews
description: "Monitor birdseye for documentation in review and respond to human comments in real-time. Birdseye is a local web app for reviewing research, plans, and other markdown files in thoughts/ directories -- NOT code review. Use when asked to watch reviews, respond to feedback, or monitor birdseye conversations."
---

# Monitor Birdseye Reviews

## What is Birdseye?

Birdseye is a local web app that **only** operates on markdown files inside
`thoughts/` directories (e.g. `thoughts/shared/plans/api-design.md`). It is
for reviewing research documents, implementation plans, guides, and similar
artifacts -- NOT source code or PRs. Think of it as a collaborative document
review system where humans and AI agents can have conversations anchored to
specific text in these markdown files.

## Workflow

1. Call `birdseye_files_in_review` for the current project to discover
   documentation files awaiting review.
2. For each file in review:
   a. Read the file content (a markdown file under `thoughts/`) so you have
      full context
   b. Call `birdseye_list_threads` with status "open" to find unaddressed threads
   c. Read each thread with `birdseye_read_thread`
   d. Reply thoughtfully to comments you haven't yet responded to
   e. Resolve threads where you've fully addressed the feedback
3. Poll `birdseye_files_in_review` every 20 seconds.
4. When new threads appear, read and respond to them.
5. When a file's review is completed (disappears from the list), stop
   monitoring that file and report to the user.
6. When no files remain in review, report completion and stop.

## Guidelines

- The files being reviewed are markdown documents (research, plans, etc.) in
  `thoughts/` directories, not source code
- Read the full file content before responding to comments, so you have context
- Be concise in replies -- the human is reading these in a side panel
- If a comment asks you to change the file, make the change and reply confirming
- If you disagree with feedback, explain your reasoning rather than silently ignoring
- Keep polling even when idle to maintain the "agent active" heartbeat in the UI
