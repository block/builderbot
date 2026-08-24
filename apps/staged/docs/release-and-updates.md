# Staged Release And Auto-Update Runbook

This app ships a stable macOS arm64 release channel with:

1. a signed DMG for manual installs
2. an in-app updater feed served from a fixed GitHub release tag

## Stable channel URLs

- Version tags: `staged/vX.Y.Z` (immutable, one per release)
- Stable alias tag: `staged-latest` (rolling, always points at the newest stable release)
- Stable DMG URL: `https://github.com/block/builderbot/releases/download/staged-latest/Staged-latest-aarch64.dmg`
- Stable updater URL: `https://github.com/block/builderbot/releases/download/staged-latest/latest.json`

Stable alias assets on `staged-latest`:

- `latest.json` — the updater feed (contains the `darwin-aarch64` entry)
- `Staged.app.tar.gz` — the updater archive
- `Staged.app.tar.gz.sig` — the Tauri updater signature for the archive
- `Staged-latest-aarch64.dmg` — the manual installer

The fixed `staged-latest` tag is required because this repository publishes releases for multiple apps. GitHub's global `releases/latest` endpoint is not channel-safe in this monorepo.

## Stable update contract

These are the load-bearing decisions for the stable channel. Changes to any of them
need explicit review; the enforcement points listed below must stay true.

### 1. Only official release builds enable updating

The in-app updater is active only in official signed release builds. Two independent
gates must both be satisfied:

- **Frontend gate:** the renderer's update loop runs only when
  `VITE_UPDATER_ENABLED === 'true'`, the build is not a dev build
  (`import.meta.env.DEV`), and the app is running under Tauri. Only
  `.github/workflows/staged-release.yml` sets `VITE_UPDATER_ENABLED=true`.
- **Native gate:** `src-tauri/src/lib.rs` registers `tauri-plugin-updater` only when
  the resolved Tauri config contains a non-empty `plugins.updater.pubkey`. The base
  `src-tauri/tauri.conf.json` intentionally has **no** `plugins.updater` section, so
  development, source, and web builds cannot contact the updater even if the frontend
  gate were misconfigured.

Development, source-built, and web-only installations therefore never check for or
install updates.

### 2. The endpoint and public key are fixed at build time

The updater endpoint and public key are injected only while generating the release
configuration (`scripts/build-tauri-release-config.mjs` writes the gitignored
`src-tauri/tauri.release.conf.json` from `STAGED_UPDATER_PUBLIC_KEY` and
`STAGED_UPDATER_ENDPOINT`). Neither value is committed to source, and the release
workflow pins the endpoint to the fixed `staged-latest/latest.json` URL. Do not use
GitHub's global `releases/latest` endpoint.

### 3. The renderer cannot supply update URLs or keys

The trust boundary lives in the native build configuration. The renderer's
capability grants in `src-tauri/capabilities/default.json` are limited to
`updater:default`, `dialog:default`, and `process:allow-restart` — enough to check,
download, install, and relaunch against the build-time endpoint, but no permission
surface accepts an arbitrary endpoint or public key from the renderer. Keep it that
way: no updater-related backend commands should accept URLs or keys as arguments.

### 4. Updates are forward-only and require user consent

Tauri's version comparison only offers versions newer than the running app; there is
no downgrade or rollback path (recover a bad release by publishing a forward fix, or
manually reinstalling an immutable versioned DMG). An update is downloaded and
installed only after the user explicitly accepts the native confirmation dialog.
Declining leaves the app fully usable, and the same version is not re-prompted
within an app session.

### 5. Failed checks never block the app

Update checks run in the background (at startup and every 15 minutes) with a
timeout. Network failures, malformed or unsigned feeds, and install errors are
logged and treated as recoverable; they must never block the window, workspace, or
database from opening.

### 6. Database compatibility is owned by SQLite migrations, not the updater

The existing SQLite migration flow and the "database too new" protection remain
authoritative. The updater does not read, write, or gate on database state, and a
storage incompatibility is surfaced as a compatibility failure — not as an ordinary
update notification.

### 7. Naming conventions are stable

The app identifier, product name, version tag scheme (`staged/vX.Y.Z`), stable
alias (`staged-latest`), and asset names (`Staged.app.tar.gz`,
`Staged.app.tar.gz.sig`, `Staged-latest-aarch64.dmg`, `latest.json`) are part of
the update contract. Renaming any of them strands existing installations, as does
rotating the updater signing key without a migration path.

## One-time setup

Generate updater signing keys once:

```bash
cd /Users/wesb/dev/builderbot/apps/staged
pnpm exec tauri signer generate -- --write-keys ~/.tauri/staged-release.key
```

Repository secrets required by `.github/workflows/staged-release.yml`:

- `STAGED_UPDATER_PUBLIC_KEY`: contents of `~/.tauri/staged-release.key.pub`
- `TAURI_SIGNING_PRIVATE_KEY`: contents of `~/.tauri/staged-release.key`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: password for the private key
- `APPLE_CERTIFICATE`: base64-encoded Developer ID Application certificate (`.p12`)
- `APPLE_CERTIFICATE_PASSWORD`: password for the certificate
- `APPLE_SIGNING_IDENTITY`: Developer ID Application identity name
- `APPLE_ID`: Apple ID email used for notarization
- `APPLE_PASSWORD`: app-specific password for notarization
- `APPLE_TEAM_ID`: Apple team identifier

## Release flow

1. From `apps/staged` on `main`, run `just prepare X.Y.Z`.
2. That command:
   - creates a release branch
   - updates `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`
   - regenerates `src-tauri/Cargo.lock`
   - commits and pushes the branch
   - opens the release PR
3. Merge the release PR to `main`.
4. From `apps/staged` on `main`, run `just release X.Y.Z`.
5. GitHub Actions builds the signed release, publishes the versioned release at `staged/vX.Y.Z`, and refreshes the fixed `staged-latest` updater and DMG aliases.

The tag trigger still expects the git tag to match the staged app version in:

- `apps/staged/package.json`
- `apps/staged/src-tauri/tauri.conf.json`
- `apps/staged/src-tauri/Cargo.toml`
- `apps/staged/src-tauri/Cargo.lock` (`[[package]] name = "Staged"` entry)

## Local verification

Generate the release config locally:

```bash
cd /Users/wesb/dev/builderbot/apps/staged
STAGED_UPDATER_PUBLIC_KEY="$(cat ~/.tauri/staged-release.key.pub)" \
STAGED_UPDATER_ENDPOINT="https://github.com/block/builderbot/releases/download/staged-latest/latest.json" \
pnpm run tauri:release:config
```

Then build a local release bundle:

```bash
cd /Users/wesb/dev/builderbot/apps/staged
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/staged-release.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="<your-key-password>"
export APPLE_SIGNING_IDENTITY="Developer ID Application: <team name> (<team id>)"

VITE_UPDATER_ENABLED=true \
pnpm exec tauri build --target aarch64-apple-darwin --config src-tauri/tauri.release.conf.json
```

The workflow publishes two kinds of assets:

- versioned release assets on `staged/vX.Y.Z`
- stable alias assets on `staged-latest`:
  - `latest.json`
  - `Staged.app.tar.gz`
  - `Staged.app.tar.gz.sig`
  - `Staged-latest-aarch64.dmg`
