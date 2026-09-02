-- Which actions a card header surfaces as their own button, and what icon each
-- one shows. Until now the header's single button was the *implicit* first
-- run-type action by sort order, decided in the frontend and stored nowhere;
-- `pinned` makes that an explicit, per-action choice of any type and any count.
ALTER TABLE repo_actions ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;

-- A kebab-case Lucide icon name ("rocket", "flask-conical"). NULL means "the
-- default icon for this action type" — which is how a detected run action keeps
-- its play button without detection ever having to pick an icon.
ALTER TABLE repo_actions ADD COLUMN icon TEXT DEFAULT NULL;

-- Every context's current implicit main action — the first run-type action by
-- sort order, exactly what the header promoted to a button — becomes explicitly
-- pinned, so no database arrives here having silently lost its run button. The
-- icon stays NULL, which renders as the same play icon it had. `sort_order` has
-- never been unique, so the tie-break continues deterministically rather than
-- leaving the pick to whatever order the scan happens to return.
UPDATE repo_actions SET pinned = 1 WHERE id IN (
    SELECT id FROM (
        SELECT id, ROW_NUMBER() OVER (
            PARTITION BY context_id
            ORDER BY sort_order ASC, created_at ASC, id ASC
        ) AS rn
        FROM repo_actions WHERE action_type = 'run'
    ) WHERE rn = 1
);
