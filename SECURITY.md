# Security policy

BOTÓN saves public Internet content to disk. Content from a legitimate archive can
still be malformed, mislabelled, copyrighted, disturbing, or unsafe to open. The
application is a downloader, not a malware scanner or a content moderation service.

## Supported versions

Security fixes target the latest code on the default branch and the latest release
when one is published. Older versions do not receive backported fixes. A passing
build does not constitute an independent security audit.

## Report a vulnerability privately

Use the repository's **Security → Report a vulnerability** option when available:
<https://github.com/fkxtr/The-Button/security/advisories/new>.

If private reporting is unavailable, open a public issue containing only a request
for a private reporting channel. Do not include a proof of concept, exploitable URL,
sensitive files, or vulnerability details in that issue. Wait for a maintainer to
provide a private channel. There is no promised response time.

In a private report, include the affected commit or version, operating system,
reproduction steps, expected security boundary, impact, and a minimal safe example.
Please allow time to investigate and prepare a fix before public disclosure.

## Expected boundaries

- Discovery uses specific public sources, not arbitrary user-supplied URLs.
- Rust validates HTTPS destinations and every redirect before following them.
- Remote metadata and filenames are untrusted. Final names remain inside the
  Downloads directory, and existing files are never intentionally overwritten.
- Downloads are streamed with time and size limits and completed through temporary
  files. Cancellation and failures remove active temporary files.
- File extensions, declared media types, available signatures, sizes, and source
  checksums are checked where supported. None of these prove that content is safe.
- The app never executes, previews, extracts, mounts, or automatically opens a file.
- Frontend code cannot choose download paths or arbitrary network destinations.

Trust still depends on the operating system, TLS roots, public source operators,
and any explicitly configured network proxy. A source checksum can detect damaged
transfers; a malicious source can publish a malicious file and a matching checksum.
A process killed forcibly or a machine losing power can leave a temporary file;
this is different from an ordinary cancellation.

## Handling downloaded files

Treat every download as untrusted. Do not execute unknown files or disable your
operating system's protections to open them. HTML and PDF documents can contain
active or malicious content even though BOTÓN does not render them. Inspect files
with tools you trust, and respect the source's copyright and licensing conditions.

See [the architecture notes](docs/architecture.md) for the implementation's trust
boundaries and [randomness](docs/randomness.md) for source selection limits.
