# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Schema flattening no longer expands exponentially: a document with two or more self-references under one node (a binary-tree shape) blew up to billions of labels and exhausted memory during `check`. Expansion is now memoized per `$ref` and label and hard-capped at 100,000 facts per pass, recording one deterministic `truncated` marker fact and surfacing a `fact expansion stopped` warning in `check` output.
- `add` is now idempotent as documented: re-adding an already-tracked name with the same URL and headers keeps the stored baseline and exits `0`; a different URL or headers exits `1` with guidance to `remove` the target and add again. The composite Action runs `add` unconditionally instead of substring-matching the old error text.

## [0.3.0] - 2026-10-27

### Added
- Prebuilt release binaries: version tags now trigger builds for Linux x86_64 (gnu), macOS (Apple Silicon), and Windows x86_64, attaching `apiwatch-{target}.tar.gz` tarballs with SHA-256 checksums to the release.
- The composite Action prefers the prebuilt binary matching its version tag and falls back to building from source when it is not available.

## [0.2.0] - 2026-10-27

### Added
- Composite GitHub Action (`action.yml`) that installs API Watch, re-adds a `spec` target idempotently on a cold cache, and runs `check` — the repository dogfoods it against `examples/spec.yaml`.
- `examples/spec.yaml`, a minimal OpenAPI 3 document used by the Dogfood workflow and local-file examples.

## [0.1.0] and earlier

See the [GitHub releases](https://github.com/hix-pro/api-watch/releases) for the
history of earlier versions.
