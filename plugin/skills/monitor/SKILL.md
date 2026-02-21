---
name: monitor
description: "Monitor and respond to penpal document review comments on markdown files in thoughts/ directories."
---

# Penpal Document Review

## What is Penpal?

Penpal is a local web app for reviewing markdown files inside `thoughts/`
directories (e.g. `thoughts/shared/plans/api-design.md`). It is for reviewing
research documents, implementation plans, guides, and similar artifacts -- NOT
source code or PRs. Humans and AI agents can have conversations anchored to
specific text in these markdown files.

## How Review Works

A file is "in review" whenever it has open comment threads. There is no explicit
review request step -- creating a comment thread on a file automatically puts it
in review. Resolving all threads on a file removes it from review.

## Monitoring Workflow

1. If you don't already know your project name, call `penpal_find_project`
   with your working directory to get it. Use this name for all subsequent
   penpal tool calls.
2. Call `penpal_files_in_review` for the project to discover files with open
   comment threads.
3. For each file in review:
   a. Read the file content (the markdown file under `thoughts/`) so you have
      full context.
   b. Call `penpal_list_threads` with status "open" to find unaddressed
      threads.
   c. Read each thread with `penpal_read_thread`.
   d. Reply thoughtfully to comments you haven't yet responded to.
4. Call `penpal_wait_for_changes` in a loop to wait for new activity. This
   tool blocks for up to 30 seconds and returns immediately when a comment is
   created, replied to, resolved, or reopened. It also maintains your agent
   heartbeat automatically.
5. When the tool returns with `"changed": true`, check the returned files list
   for new or updated threads and respond to them.
6. When a file has all threads resolved (disappears from the list), stop
   monitoring that file and report to the user.
7. When no files remain in review, report completion and stop.

## Guidelines

- Read the full file content before responding to comments so you have context
- Be concise in replies -- the human is reading these in a narrow side panel
- ALWAYS reply to every thread you work on. When a comment asks for changes,
  make the changes and then reply confirming what you did. When you investigate
  something, reply with what you found. The human sees a "typing" indicator
  while you work -- the reply is what tells them you're done.
- If you disagree with feedback, explain your reasoning rather than silently
  ignoring it
- Do NOT resolve threads -- only humans resolve threads. Your job is to reply
  and make changes, not to decide when a conversation is done
- Keep calling `penpal_wait_for_changes` even when idle -- this maintains the
  "agent active" heartbeat indicator in the penpal UI
- Do NOT stop monitoring just because there are no open threads; the human may
  add new comments at any time

## Available MCP Tools

| Tool | Purpose |
|------|---------|
| `penpal_find_project` | Get the project name for your working directory. Call first if you don't know it. |
| `penpal_wait_for_changes` | Block until comments change (or 30s timeout). Returns files in review. Use in a loop. |
| `penpal_files_in_review` | List all files with open comment threads for a project |
| `penpal_list_threads` | List comment threads on a file (optionally filter by status) |
| `penpal_read_thread` | Read a full comment thread with all replies |
| `penpal_create_thread` | Create a new comment thread anchored to text |
| `penpal_reply` | Reply to an existing thread |
