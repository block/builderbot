# Staged Store Migrations

This directory contains the SQLite schema snapshot and future append-only
migrations for the Staged store.

## Current model

- `baseline.sql` is the fresh-install schema snapshot.
- `baseline.sql` is **not** part of the ordered migration chain.
- Fresh databases bootstrap directly from `baseline.sql`.
- Older versioned databases upgrade through the compatibility steps in
  `../migrations.rs` until they reach the baseline schema version.
- New schema changes after the baseline are added as ordered migrations and
  registered in `../migrations.rs`.

The current baseline version is defined by `BASELINE_SCHEMA_VERSION` in
`../migrations.rs`. The latest supported schema version is `SCHEMA_VERSION` in
`../mod.rs`.

## Why `baseline.sql` is not `0001_baseline.sql`

Migration ordering is not discovered from filenames. Ordering is enforced by
the Rust migration registry in `../migrations.rs`.

That means:

- `baseline.sql` can keep a stable, descriptive name.
- Future migrations should use ordered filenames like `0023_add_foo.sql`.
- The baseline snapshot should not be renamed to `0001_*.sql`, because it is
  not replayed like a normal migration step.

## Adding a new migration

For a normal schema change after the baseline:

1. Add a new SQL file in this directory, for example `0023_add_foo.sql`.
2. Bump `SCHEMA_VERSION` in `../mod.rs`.
3. Register the migration in `MIGRATIONS` in `../migrations.rs` with the same
   version number.
4. Add or update migration coverage in `../migration_tests.rs`.
5. Run:

```sh
source ./bin/activate-hermit && cargo test --manifest-path apps/staged/src-tauri/Cargo.toml store::
```

Use a Rust migration instead of pure SQL only when the upgrade needs
conditional backfills or nontrivial data reshaping.

## When to update `baseline.sql`

Do **not** edit `baseline.sql` for every schema change.

The normal flow is:

- keep `baseline.sql` as the snapshot for the current baseline version
- append new ordered migrations after that baseline

Only update `baseline.sql` when intentionally squashing the migration history
into a newer baseline. If you do that, also advance `BASELINE_SCHEMA_VERSION`
and trim any legacy upgrade steps that are no longer supported.
