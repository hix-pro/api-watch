# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Composite GitHub Action (`action.yml`) that installs API Watch, re-adds a `spec` target idempotently on a cold cache, and runs `check` — the repository dogfoods it against `examples/spec.yaml`.
- `examples/spec.yaml`, a minimal OpenAPI 3 document used by the Dogfood workflow and local-file examples.

## [0.1.0] and earlier

See the [GitHub releases](https://github.com/hix-pro/api-watch/releases) for the
history of earlier versions.
