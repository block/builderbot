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

The config generator validates both values: the endpoint must be an HTTPS,
credential-free URL pointing at a `latest.json` feed, and the public key must be
the single-line base64 value produced by `tauri signer generate`. Validation returns
the value that is written, so surrounding whitespace (typically a trailing newline
from `$(cat …)` or a CI secret) is stripped rather than written into the config —
Tauri's base64 decoder would reject an untrimmed key at runtime, producing a signed
build whose updater cannot verify anything. Official release builds (the workflow
sets `STAGED_OFFICIAL_RELEASE=true`) **fail closed** if either value is missing,
rather than silently producing a release configuration without updater settings.
Unofficial local builds may still generate a config without updater settings for
testing.

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
Declining leaves the app fully usable, and background checks do not re-prompt the
same version within an app session. An explicit manual check from Settings always
re-prompts, so a declined update can still be installed without restarting.

### 5. Failed checks never block the app

Update checks run in the background (at startup and every 15 minutes) with a
timeout that is applied both to the native request and to the renderer waiting on
it. A check that times out holds off the next one only until it settles, and that
wait is itself bounded — a native call that never settles cannot disable updates
for the rest of the session. Network failures, malformed or unsigned feeds, and
install errors are logged and treated as recoverable; they must never block the
window, workspace, or database from opening. Every update handle the renderer
obtains is released (Tauri's `Update` is a native resource), including handles for
updates that are declined, suppressed, or discarded after a timeout.

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

## Multiple windows

Update prompts, installs, and relaunches are app-wide, so exactly one window owns the
updater: the backend grants a single-owner lease (`claim_updater_ownership`) and
only that window runs checks, shows the consent dialog, and installs. Every other
window mirrors the owner's status over frontend events
(`src/lib/services/updaterBridge.ts`): the owner broadcasts `updater-state-sync` on
every state change and in reply to a mirror's `updater-state-request`, and a
"Check for updates" click in a mirror window sends `updater-check-request`, which
the owner runs as a manual check. When the owner window is destroyed, the native
`Destroyed` hook releases the lease and broadcasts `updater-owner-available`; the
surviving window that wins the new claim stops mirroring and starts the service.

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

## Publication validation

The publication scripts (`scripts/publish-updater-to-github-release.mjs` and
`scripts/publish-dmg-to-github-release.mjs`) share validation helpers in
`scripts/release-checks.mjs` and enforce the stable contract before and after
upload:

- only the `stable` channel is accepted
- the release version must be plain `X.Y.Z` and match the `staged/vX.Y.Z` tag
- the updater archive and its `.sig` file must exist, with a nonempty signature
- the version embedded in the archive (`CFBundleShortVersionString` in its
  top-level `<Name>.app/Contents/Info.plist`) must match the released version.
  `Staged.app.tar.gz` carries no version in its name, so this is what stops a
  stale bundle left in a local `src-tauri/target` from being published, validly
  signed, under a newer version
- `latest.json` is always generated by the publisher from that verified archive
  (any `latest.json` already in the bundle directory is ignored) and validated
  **before** anything is uploaded to the alias: the feed version matches the
  released version, the `darwin-aarch64` platform record is present, its
  signature is nonempty, and its URL points at the stable alias archive over plain
  HTTPS
- after upload, the alias release must contain `latest.json`, the archive, and the
  signature (and the DMG releases their respective assets), and every published URL
  must be reachable before the workflow reports success (set
  `SKIP_REACHABILITY_CHECK=1` to skip the network probe)

Set `DRY_RUN=1` to validate artifacts and print the `gh` commands without touching
real releases. The release-script tests (`scripts/release-checks.test.ts` and
`scripts/release-scripts.test.ts`) exercise these checks against dry-run fixtures
and run as part of `pnpm test`.

## The stagedctl sidecar

The `stagedctl` control CLI ships inside the bundle at
`Staged.app/Contents/MacOS/stagedctl` as a Tauri sidecar
(`bundle.externalBin` in `tauri.conf.json`), so the codesign/notarize step
signs it along with the app. `scripts/prepare-stagedctl-sidecar.sh` builds
and stages it under `src-tauri/binaries/` before every tauri build — it runs
automatically from the before-dev/before-build hooks and the `just
dev`/`lint`/`test` recipes, so no release-workflow step is needed. App
updates replace the sidecar together with the app; `install.sh` symlinks
`/usr/local/bin/stagedctl` into the installed bundle so a user-visible CLI
tracks those updates.
