-- "Push after current work": a push armed on a branch that fires once the
-- branch and its project have nothing running, disconnected or queued. It is
-- a flag on the branch rather than a queued push row because a queued row
-- sits in the branch FIFO (it would block work queued behind it, and the queue
-- cannot see project sessions); see `prs::fire_deferred_push_if_idle`.
--
-- `deferred_push` is one of `push`, `force`, `push-no-verify`,
-- `force-no-verify`; NULL means nothing is armed. `deferred_push_requested_at`
-- is when it was armed, in milliseconds since the epoch.

ALTER TABLE branches ADD COLUMN deferred_push TEXT NULL;
ALTER TABLE branches ADD COLUMN deferred_push_requested_at INTEGER NULL;
