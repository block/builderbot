# Staged Store Migrations

This directory contains the append-only SQL migrations for the Staged store.
They are discovered by `rusqlite_migration` from this folder and tracked using
SQLite `user_version`.

## Current model

- Each migration lives in a numbered subdirectory such as `0001-baseline/`.
- The required file is `up.sql`.
- Fresh databases apply migrations in order on first open.
- Only databases created after this migration system landed are migrated in
  place. Older Staged databases are treated as incompatible and reset.
- Future schema changes should be added as new numbered directories.
- `app_metadata` is application-owned metadata for UX messaging; it is not used
  to decide which migrations have run.

## Adding a new migration

1. Add a new directory in this folder, for example `0002-add-foo/`.
2. Add an `up.sql` file inside it.
3. Add or update migration coverage in `../migration_tests.rs`.
4. Run:

```sh
source ./bin/activate-hermit && cargo test --manifest-path apps/staged/src-tauri/Cargo.toml store::
```

Prefer SQL migrations. Only reach for Rust around migrations when the library
cannot express the needed behavior.
