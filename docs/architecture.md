# Architecture

BOTÓN is a Tauri 2 desktop application. Its frontend is bundled TypeScript, HTML,
and CSS; Rust owns discovery, networking, validation, cancellation, and files.
There is no hosted application backend, account database, or telemetry service.

## Frontend and native boundary

The main window initially measures 520 × 360 pixels. `src/main.ts` listens for
native state events and invokes a small command surface:

| Command | Purpose |
| --- | --- |
| `start_download` | Start one discovery-and-download task. |
| `confirm_download` | Accept or decline the pending large/unknown-size transfer. |
| `cancel_download` | Cancel active discovery, confirmation, or transfer. |

Frontend callers cannot supply arbitrary URLs, output paths, or shell commands.
The bundled main window has only the Tauri event and application command
permissions it needs. The content security policy blocks remote UI scripts,
frames, and external resource loading. Remote archive metadata is displayed only
as text, never inserted as HTML.

The UI follows:

```text
idle → discovering → [confirming] → downloading → complete → idle
                   ↘ error → idle
```

Cancellation returns to idle. The native backend also guards against overlapping
jobs, so disabling the button is not the only concurrency control. Completion and
error messages are short-lived. The Web Audio click is synthesized locally in
response to user interaction, and motion styling observes the operating system's
reduced-motion preference.

## Native modules

- `sources.rs`: public source adapters and typed candidate metadata. The sampler
  owns source, media-group, and within-item choices.
- `network.rs`: restricted HTTPS client, redirect policy, bounded JSON metadata,
  proxy support, and address validation.
- `download.rs`: response preparation, confirmation thresholds, streaming writes,
  file validation, checksums, cancellation, and collision-safe finalization.
- `app.rs`: Tauri commands, background-task ownership, state events, the standard
  Downloads path, and application lifecycle.

The `desktop` Cargo feature is enabled by default. Disabling it lets core tests
and the explicit network smoke example run without linking a graphical webview.
This helps isolate downloader failures from desktop-system dependencies.

## Network trust boundary

Only HTTPS URLs on specific source-owned hosts are accepted. Credentials,
fragments, IP-literal destinations, nonstandard ports, and unknown hosts are
rejected. Redirects are handled manually: every target is validated, source
families cannot change mid-request, and at most five hops are followed.

For direct connections, a custom resolver rejects nonpublic addresses before
returning socket addresses to the HTTP client. If an explicitly configured proxy
is used, the gateway can be a private address; URL restrictions still apply, while
remote destination resolution depends on that proxy. The app honors the system's
configured TLS trust and does not disable certificate verification.

Metadata responses are limited to 2 MiB. Discovery has at most six attempts and a
90-second deadline. File transfer has a 20-second no-progress timeout and a
15-minute total deadline. Source failures are visible errors, not a reason to
broaden the host allowlist.

## Download lifecycle

1. Discover a candidate with a URL, filename, and optional type, size, and SHA-1.
2. Validate the response status, transfer encoding, size, and media type. Ask the
   user before an unknown-size transfer or a file larger than 100 MiB proceeds.
3. Resolve the operating system's Downloads directory and check free space,
   including a 16 MiB reserve. Create a unique private `.boton-*.part` file in that
   directory; stream chunks to it while checking cancellation and byte limits.
4. Maintain SHA-1 and SHA-256 during streaming. Check declared lengths and the
   source SHA-1 when available. SHA-256 is returned in the internal receipt for
   validation; a locally computed hash alone is not external integrity evidence.
5. Check the initial file signature against the permitted type. Explicit executable
   and archive signatures are rejected. Text checking examines a bounded prefix,
   not an exhaustive parser. These checks do not establish that content is benign.
6. Flush the file and use no-clobber finalization in the same directory. When the
   name already exists, try a numbered suffix, preserving the original file.

The 2 GiB ceiling is checked against both metadata and actual streamed bytes.
Free space is checked again during larger transfers. Filenames become single safe
components, are length-bounded, and avoid Windows reserved device names. A file
with a misleading or unrecognized binary signature is rejected rather than
silently renamed into a type the application did not intend to accept.

Ordinary errors and cancellation drop the temporary file through RAII. Closing the
application cancels its active task. A forced process kill or power failure can
leave a partial file; completed files are never automatically opened or removed.

Checksums protect against accidental transfer changes when independent metadata
is available. SHA-1 is retained for compatibility with archive metadata, not as a
modern authenticity signature. Source metadata and source bytes share a trust
origin; matching them does not make a malicious source safe.

## Validation and packaging

Default tests use deterministic fixtures and do not contact public archives. The
frontend tests cover the state machine; Rust tests exercise sampling, metadata,
URL policy, filename handling, validation, and file lifecycle. The smoke example
is an explicit live-network check that saves to a caller-selected test directory.

Linux CI uses Ubuntu 24.04 to produce a `.deb` with a system-library baseline
appropriate for the Linux Mint 22 target. Installing a package built on a newer
host distribution can require newer libraries even if the CPU architecture
matches. The optional Windows CI job checks compilation and emits an executable;
it does not establish runtime behavior or installer quality.

Official framework references:

- [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/)
- [Calling Rust from the frontend](https://v2.tauri.app/develop/calling-rust/)
- [Calling the frontend from Rust](https://v2.tauri.app/develop/calling-frontend/)
- [Tauri capabilities](https://v2.tauri.app/security/capabilities/)
- [Content security policy](https://v2.tauri.app/security/csp/)
- [Debian packaging](https://v2.tauri.app/distribute/debian/)
