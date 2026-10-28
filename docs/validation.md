# Validation record

This records the checks executed during initial implementation on **2026-10-28**.
It describes that development environment, not certification of every supported
platform or a claim that a public release has been published.

Native validation used **Ubuntu 24.04, x86_64**, with **Rust 1.99.1** selected by
`rust-toolchain.toml`. Frontend checks used **Node.js 24.19.0**. Native build output
was kept in a separate target directory so artifacts from a different Linux
distribution were not reused.

| Check | Result |
| --- | --- |
| Frozen frontend install (`npm ci`) | Passed; lockfile installation repeated successfully. |
| TypeScript checking | Passed. |
| Frontend tests | 30 passed, including 6 DOM/IPC integration tests. |
| Frontend production build | Passed. |
| Native Clippy, all targets, warnings denied | Passed on Ubuntu 24.04. |
| Native Rust tests with the desktop feature enabled | 23 passed on Ubuntu 24.04. |
| Live public-source smoke check | Executed, blocked by the environment's proxy policy; no successful real download verified. |
| Linux release bundle | Built `boton_0.1.0_amd64.deb` (3.64 MiB); valid Debian metadata; installed successfully on Ubuntu 24.04; all runtime libraries resolved. |
| Native window, input, and animation | Installed release opened its real 520 × 360 Tauri window under Xvfb; clicking entered the busy state and then displayed the expected network error. |
| Native cancellation and close | Escape during discovery restored idle. A real `WM_DELETE_WINDOW` request during discovery exited successfully with no partial files left in Downloads. |
| Web Audio click | Captured from the application through a PulseAudio null-sink monitor: 2,775 nonzero samples, peak 1,271 in signed 16-bit PCM. This verifies synthesized output, not subjective listening quality. |
| Windows compilation and runtime | Not run. |

Commands used for the dependency, test, and build checks, from the repository
root:

```sh
npm ci
npm run typecheck
npm test
npm run build
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-targets
```

The live smoke check was invoked separately:

```sh
cargo run --locked --manifest-path src-tauri/Cargo.toml \
  --no-default-features --example smoke -- /tmp/boton-smoke
```

The proxy denied access to the public archive endpoints before a successful
discovery and file transfer could be completed. Deterministic HTTP fixtures and
unit tests exercise the download code, but they do not establish that the current
public APIs return candidates that pass every production check. Real discovery,
download completion, and source-provided checksum verification remain unverified
until the smoke check succeeds on a network that permits the documented sources.
The smoke example deliberately refuses unknown-size files and files over 100 MiB.

Packaging ran with `npm run tauri build -- --bundles deb`. The artifact SHA-256 is:

```text
bf7c4a1c0db95cb1271e63b833b37462ae720bfc5751b50978ddea34a1faadf0
```

The screenshot in the README is the installed release application, captured under
Xvfb. The headless run used `WEBKIT_DISABLE_DMABUF_RENDERER=1` for software display
compatibility; it did not disable WebKit's security sandbox. An audio monitor
recorded the actual click generated after native-window mouse input. These checks
exercise startup and the blocked-network path; they do not replace a successful
public-source download on a normal desktop.

The repository's GitHub Actions workflow is configured to run its own checks and
produce artifacts. This local record does not claim a successful remote CI run.

ZIP archives and ISO images are intentionally excluded from the current source
adapters. Windows support is prepared in configuration and an optional CI job;
Windows behavior has not been tested. Downloaded remote content is never executed
as part of validation.
