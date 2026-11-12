# Sidecar staging area

Tauri's `bundle.externalBin` (see `tauri.conf.json`) looks here for sidecar
binaries named `<name>-<target-triple>`, copies them next to the app binary
at cargo-build time, and bundles them into `Staged.app/Contents/MacOS/`.

Everything in this directory except this README is generated — run

```sh
bash scripts/prepare-stagedctl-sidecar.sh
```

from `apps/staged` to (re)stage the `stagedctl` CLI. `just dev`, `just
lint`, `just test`, and the tauri before-dev/before-build hooks all run it
automatically; if a bare `cargo build`/`test` fails with
`resource path binaries/stagedctl-<triple> doesn't exist`, run the script
(or the just recipes) once and retry.
