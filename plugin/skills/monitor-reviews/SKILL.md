---
name: monitor-reviews
user_invocable: true
description: "Review and discuss markdown documents in thoughts/ directories using birdseye. TRIGGER: always invoke this skill after creating or modifying files in thoughts/ (plans, research, guides). Also use when asked to monitor reviews, watch for feedback, respond to comments, or when user mentions \"birdseye\", \"review\", \"thoughts review\", or \"monitor reviews\"."
---

# Birdseye Document Review

## What is Birdseye?

Birdseye is a local web app for reviewing markdown files inside `thoughts/`
directories (e.g. `thoughts/shared/plans/api-design.md`). It is for reviewing
research documents, implementation plans, guides, and similar artifacts -- NOT
source code or PRs. Humans and AI agents can have conversations anchored to
specific text in these markdown files.

## How Review Works

A file is "in review" whenever it has open comment threads. There is no explicit
review request step -- creating a comment thread on a file automatically puts it
in review. Resolving all threads on a file removes it from review.

## Monitoring Workflow

1. Call `birdseye_files_in_review` for the project to discover files with open
   comment threads.
2. For each file in review:
   a. Read the file content (the markdown file under `thoughts/`) so you have
      full context.
   b. Call `birdseye_list_threads` with status "open" to find unaddressed
      threads.
   c. Read each thread with `birdseye_read_thread`.
   d. Reply thoughtfully to comments you haven't yet responded to.
   e. Resolve threads where you've fully addressed the feedback.
3. Call `birdseye_wait_for_changes` in a loop to wait for new activity. This
   tool blocks for up to 30 seconds and returns immediately when a comment is
   created, replied to, resolved, or reopened. It also maintains your agent
   heartbeat automatically.
4. When the tool returns with `"changed": true`, check the returned files list
   for new or updated threads and respond to them.
5. When a file has all threads resolved (disappears from the list), stop
   monitoring that file and report to the user.
6. When no files remain in review, report completion and stop.

## Guidelines

- Read the full file content before responding to comments so you have context
- Be concise in replies -- the human is reading these in a narrow side panel
- If a comment asks you to change the file, make the change and reply confirming
  what you changed
- If you disagree with feedback, explain your reasoning rather than silently
  ignoring it
- Keep calling `birdseye_wait_for_changes` even when idle -- this maintains the
  "agent active" heartbeat indicator in the birdseye UI
- Do NOT stop monitoring just because there are no open threads; the human may
  add new comments at any time

## Available MCP Tools

| Tool | Purpose |
|------|---------|
| `birdseye_wait_for_changes` | Block until comments change (or 30s timeout). Returns files in review. Use in a loop. |
| `birdseye_files_in_review` | List all files with open comment threads for a project |
| `birdseye_list_threads` | List comment threads on a file (optionally filter by status) |
| `birdseye_read_thread` | Read a full comment thread with all replies |
| `birdseye_create_thread` | Create a new comment thread anchored to text |
| `birdseye_reply` | Reply to an existing thread |
| `birdseye_resolve` | Mark a thread as resolved |
