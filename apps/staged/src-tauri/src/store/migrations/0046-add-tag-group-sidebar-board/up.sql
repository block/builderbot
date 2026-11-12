-- A tag group's board can be pinned to the projects sidebar: one row per
-- pinned group, under the Projects heading, opening that group's board as the
-- detail view. A catalog property, like is_icon_group, so it follows the group
-- across windows; unlike the icon flag, any number of groups may be pinned.
ALTER TABLE tag_groups
    ADD COLUMN show_in_sidebar INTEGER NOT NULL DEFAULT 0 CHECK (show_in_sidebar IN (0, 1));
