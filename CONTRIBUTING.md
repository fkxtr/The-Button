# Contributing to BOTÓN

The product is one button and one unexpected download. Changes should improve
discovery, reliability, accessibility, or clarity while preserving that focus.

## Set up

Follow the [README's source build instructions](README.md#build-from-source) for
Node.js, Rust, and native Tauri dependencies. Use the committed npm and Cargo
lockfiles. Run `npm ci`, then `npm run tauri dev` from the repository root.

The TypeScript frontend lives in `src/`; native commands, discovery, networking,
and downloading live in `src-tauri/src/`. Read
[the architecture notes](docs/architecture.md) and
[the sampling notes](docs/randomness.md) before changing those boundaries.

## Add a public file source

1. Document the source's official API, public access terms, rate limits, content
   licensing, expected redirects, and supported formats.
2. Implement an adapter that returns the existing typed candidate structure.
   Sample a broad collection rather than embedding a small list of URLs. Use
   cryptographic randomness in production and deterministic fixtures in tests.
3. Add only the specific HTTPS hosts needed by that adapter. Preserve redirect,
   address, size, timeout, MIME, and filename restrictions. Never allow arbitrary
   metadata URLs to bypass the shared network client.
4. Preserve available integrity metadata and add fixtures for missing, malformed,
   deceptive, and oversized candidates. Do not introduce live network calls into
   the default test suite.
5. Run an explicit real-source smoke check when network access permits. Verify the
   saved bytes, then report the result or exact access limitation in the pull
   request. Do not open or execute the downloaded content.
6. Update the supported-format and sampling documentation, including biases.

A ZIP or ISO adapter requires a vetted distribution source and a clear integrity
policy; accepting an archive based only on its extension is not sufficient.

## Check changes

```sh
npm run typecheck
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-targets
npm run tauri build -- --bundles deb
```

Check the native application with pointer and keyboard input. Exercise success,
failure, cancellation, confirmation, closing during a download, and reduced
motion when affected. A frontend browser preview cannot prove native commands
work. Never label an unrun platform or network check as passing.

## Pull requests and reports

Keep pull requests focused. Describe the user-visible problem, the resulting
behavior, relevant design decisions, and the commands you ran. Include screenshots
only when they capture the real implementation. Do not commit downloads, build
outputs, dependency folders, tokens, or local configuration.

For ordinary bugs, use the issue template and include the application version or
commit, operating system, reproduction steps, and relevant sanitized errors.
Do not upload downloaded files or private paths. Discuss changes that expand the
trust boundary before implementation. Follow [SECURITY.md](SECURITY.md) to report
a vulnerability privately.

By contributing original code, you agree that it can be distributed under this
project's MIT License. Third-party content must retain its own applicable terms.
