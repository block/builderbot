# Staged Release And Auto-Update Runbook

This app ships a stable macOS arm64 release channel with:

1. a signed DMG for manual installs
2. an in-app updater feed served from a fixed GitHub release tag

## Stable channel URLs

- Version tags: `staged/vX.Y.Z`
- Stable alias tag: `staged-latest`
- Stable DMG URL: `https://github.com/block/builderbot/releases/download/staged-latest/Staged-latest-aarch64.dmg`
- Stable updater URL: `https://github.com/block/builderbot/releases/download/staged-latest/latest.json`

The fixed `staged-latest` tag is required because this repository publishes releases for multiple apps. GitHub's global `releases/latest` endpoint is not channel-safe in this monorepo.

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

1. Bump the version in:
   - `apps/staged/package.json`
   - `apps/staged/src-tauri/tauri.conf.json`
   - `apps/staged/src-tauri/Cargo.toml`
   - `apps/staged/src-tauri/Cargo.lock` (`[[package]] name = "Staged"` entry)
2. Commit the version bump to `main`.
3. Push a tag named `staged/vX.Y.Z`.
4. GitHub Actions builds the signed release, publishes the versioned release at `staged/vX.Y.Z`, and refreshes the fixed `staged-latest` updater and DMG aliases.

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
