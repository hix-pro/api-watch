# API Watch

**Catch breaking changes in the APIs you depend on — before they reach production.**

[![CI](https://github.com/hix-pro/api-watch/actions/workflows/ci.yml/badge.svg)](https://github.com/hix-pro/api-watch/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/hix-pro/api-watch)](https://github.com/hix-pro/api-watch/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

`apiwatch` saves a baseline of an OpenAPI 3.x, Swagger 2.0, or JSON document, re-fetches it on a schedule or in CI, and reports exactly which parts of the contract changed. With `--fail-on-breaking`, a removed endpoint or a new required field fails the build.

One small binary. Local SQLite. No account, daemon, or cloud service.

## Why

Changelogs are incomplete, and "we'll announce breaking changes" is not a contract. API Watch compares the thing that actually defines the contract — the spec — and gives you a diff you can act on:

- **Direction-aware.** A new enum value in a request is fine; a new enum value in a response can break exhaustive clients.
- **Historical.** Every check is stored, so you can see when a field disappeared, not just that it is gone.
- **CI-ready.** Exit code `2` on breaking changes, JSON output for tooling, ETag support to avoid downloading unchanged specs.
- **Generic JSON too.** Point it at a config endpoint or feature-flag payload; it tracks added, removed, and retyped keys.

## See it

```console
$ apiwatch add https://petstore3.swagger.io/api/v3/openapi.json --name petstore
Added petstore
  URL       https://petstore3.swagger.io/api/v3/openapi.json

Next: apiwatch check petstore

$ apiwatch check petstore
petstore · OpenAPI · Swagger Petstore - OpenAPI 3.0 1.0.28
baseline saved · 515 facts · 19 endpoints · HTTP 200

$ apiwatch check petstore
petstore · OpenAPI · Swagger Petstore - OpenAPI 3.0 1.0.28
no contract changes · 515 facts · 19 endpoints · HTTP 304
  not modified (ETag matched)
```

When a spec does change:

```console
$ apiwatch check orders --fail-on-breaking
orders · OpenAPI · Orders API 1.1.0
breaking changes detected · 6 breaking · 2 compatible
  BREAKING    DELETE /orders/{id}
              endpoint removed
  BREAKING    GET /orders · 200 application/json[].total
              response property removed
  BREAKING    GET /orders · 200 application/json[].status
              new enum value may be returned: "cancelled"
  BREAKING    POST /orders · body application/json.currency
              new required request property
  COMPATIBLE  GET /orders · query cursor
              new optional parameter
  COMPATIBLE  GET /orders · 200 application/json[].status
              enum value no longer returned: "pending"

$ echo $?
2
```

## Install

Building from source requires **Rust 1.85+**. SQLite is bundled; there are no system dependencies.

```bash
git clone https://github.com/hix-pro/api-watch.git
cd api-watch
cargo install --path . --locked
```

Or build without installing:

```bash
cargo build --release
./target/release/apiwatch --help
```

Prebuilt binaries — every version ships tarballs for Linux x86_64 (glibc), macOS (Apple Silicon), and Windows x86_64, each with a SHA-256 checksum:

```bash
curl -LO https://github.com/hix-pro/api-watch/releases/latest/download/apiwatch-x86_64-unknown-linux-gnu.tar.gz
tar xzf apiwatch-x86_64-unknown-linux-gnu.tar.gz
install apiwatch-x86_64-unknown-linux-gnu/apiwatch /usr/local/bin/
```

## Quick start

```bash
# Track a remote spec
apiwatch add https://petstore3.swagger.io/api/v3/openapi.json --name petstore

# Track a local file (relative paths and file:// both work)
apiwatch add ./openapi.yaml --name my-api

# Record the first snapshot
apiwatch check my-api

# Check everything and gate CI on breaking changes
apiwatch check --fail-on-breaking

# Compare a candidate spec to the saved baseline without writing history
apiwatch verify my-api ./openapi.next.yaml --fail-on-breaking

# Inspect
apiwatch list
apiwatch history petstore --limit 20
```

Specs behind auth use static headers:

```bash
apiwatch add https://internal.example.com/openapi.json \
  --name internal \
  -H "Authorization: Bearer $API_TOKEN"
```

Headers are stored in the local database so later checks can reuse them. **They are not encrypted** — use a scoped, rotated token, and keep the database private. See [Storage and security](#storage-and-security).

## Commands

| Command | Description |
| --- | --- |
| `apiwatch add <source>` | Track a URL or local path. `--name` overrides the derived name; `-H/--header` adds a request header. Re-adding an already-tracked name with the same URL and headers is a no-op that keeps the stored baseline; a different URL or headers exits `1` (use `remove` first to replace the target). |
| `apiwatch check [name]` | Fetch and compare. No name checks every target. `--json`, `--sarif`, `--fail-on-breaking`. |
| `apiwatch verify <name> <source>` | Compare a candidate URL or local file against the saved baseline without recording a check or snapshot. `-H/--header`, `--json`, `--sarif`, `--fail-on-breaking`. |
| `apiwatch list` | Show targets and their last result. Alias: `ls`. `--json`. |
| `apiwatch history <name>` | Show past checks. `--limit` (default 10, max 1000), `--json`. |
| `apiwatch remove <name>` | Stop tracking and delete snapshots, checks, and changes. Alias: `rm`. |

Global option: `--db <path>` selects the database (default `~/.apiwatch/apiwatch.db`, env `APIWATCH_DB`).

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | All targets checked. Includes compatible-only changes, and breaking changes **without** `--fail-on-breaking`. |
| `1` | Fetch, parse, or usage error (also recorded in history for `check`). |
| `2` | Breaking changes found **and** `--fail-on-breaking` was set. |

This distinction matters: a tool that always exits non-zero on changes cannot be used for monitoring without also failing on safe additions.

## Verify a candidate spec

`check` writes to history and advances the baseline. That is right for a scheduled watch, but wrong for a pull request: the candidate should be compared against the baseline on `main`, not replace it. `verify` does exactly that.

```bash
# Baseline already recorded by an earlier check
apiwatch verify orders ./api/openapi.yaml --fail-on-breaking
```

`verify` accepts a URL or a local path, reads the latest stored snapshot for the target, and prints the same text, JSON, or SARIF report as `check`. It never records a check and never writes a snapshot. `--fail-on-breaking` exits `2` for breaking changes; without it, the command reports and exits `0`. A missing baseline exits `1` with the command to run first.

In a pull request workflow, restore or cache the database from the base branch, then verify the spec in the checkout:

```yaml
- uses: actions/cache/restore@v4
  with:
    path: ~/.apiwatch
    key: apiwatch-main
- name: Verify the API contract
  run: apiwatch verify orders ./openapi.yaml --fail-on-breaking
```

Add `--sarif` to upload the result to code scanning. Annotations point at the candidate file, not the stored target URL.

## What counts as breaking

API Watch compares a flattened set of contract facts and applies different rules to requests and responses.

| Change | Classification |
| --- | --- |
| Endpoint removed | **Breaking** |
| Endpoint added | Compatible |
| Request parameter or body property removed | **Breaking** |
| Request or response media type removed | **Breaking** |
| Request or response media type added | Compatible |
| New required request parameter or property | **Breaking** |
| New optional request parameter or property | Compatible |
| Request field becomes required | **Breaking** |
| Request field becomes optional | Compatible |
| Response property removed | **Breaking** |
| Response property added | Compatible |
| Response header removed | **Breaking** |
| Response header added | Compatible |
| Response field is no longer guaranteed | **Breaking** |
| Response field becomes guaranteed | Compatible |
| Field type changes (`string` → `integer`, etc.) | **Breaking** |
| Request enum value removed | **Breaking** |
| Request enum value added | Compatible |
| Request value newly restricted to an enum | **Breaking** |
| Request enum restriction removed | Compatible |
| Response enum value added | **Breaking** |
| Response enum value removed | Compatible |
| Response value newly restricted to an enum | Compatible |
| Response enum restriction removed | **Breaking** |
| Format added to a request field (`string` → `uuid`) | **Breaking** |
| Format removed from a response field | **Breaking** |
| Descriptions, examples, summaries, tags | Ignored |

For generic JSON documents, removed keys and changed types are breaking; added keys are compatible. Arrays are treated as homogeneous and compared using their first element.

A new endpoint brings its whole contract at once, so its required parameters do not fail `--fail-on-breaking`. When an endpoint, response, request body, or media type is added or removed, API Watch reports the owning change once instead of listing every nested fact. Adding a media type to an existing required body is compatible: requiredness belongs to the body, not to each representation.

## How it works

1. **Fetch** the URL or read the local file. Previous ETags are sent as `If-None-Match`; a `304` records an unchanged check without downloading the spec. HTTP responses and local files are capped at **25 MB**; HTTP requests time out after **30 seconds**. Custom headers are never forwarded across a cross-origin redirect.
2. **Parse** JSON or YAML and detect OpenAPI/Swagger. Local `$ref`s are resolved (up to 32 levels).
3. **Flatten** the document into facts keyed by a stable label, for example `POST /pets · body application/json.name` or `GET /pets · 200 application/json[].status`. Self-referential schemas are bounded: a `$ref` is expanded once per label, and runaway expansion stops at a 100,000-fact budget, recording a deterministic `truncated` marker that `check` surfaces as a warning.
4. **Diff** the new facts against the last stored baseline and classify each change by direction.
5. **Store** the snapshot, check, and individual changes in SQLite. Only changed-or-first documents store a new snapshot; unchanged checks store just the check row.

```text
src/
├── cli.rs      add / remove / list / check / history
├── fetch.rs    HTTP, ETag, limits, local files
├── spec.rs     JSON/YAML parsing, OpenAPI flattening, $ref resolution
├── diff.rs     breaking vs compatible rules
├── db.rs       SQLite schema and history
├── report.rs   terminal and JSON output
└── style.rs    ANSI colors with NO_COLOR support
```

## CI

The key detail: CI needs a persistent baseline. Keep the SQLite database in a cache, a private artifact store, or a self-hosted persistent volume. If the cache is cold and the database is empty, `apiwatch check` reports `no targets yet` and exits `1` — re-add the targets with `apiwatch add`, and the first check records a baseline and exits `0`.

## GitHub Action

The repository ships a composite action that installs API Watch and runs `check` — no Rust setup in your workflow:

```yaml
- uses: hix-pro/api-watch@v0.4.0
  with:
    spec: openapi.yaml        # re-added automatically when the cache is cold
    name: my-api
    check-args: --fail-on-breaking
```

Passing `spec` makes cache eviction self-healing: `add` is idempotent, so on a warm cache re-adding the already-tracked name with the same URL and headers is a no-op that keeps the stored baseline (a different URL or headers fails the step, since the name would point at another contract), and on a cold cache it restores the tracking entry so the first check records a fresh baseline and exits `0`.

The SARIF support in this section is available from `v0.4.0`. Inputs: `spec`, `name`, `check-args` (for example `--fail-on-breaking`), `db-path` (default `~/.apiwatch/apiwatch.db`, with `~` expanded for you), `sarif` (default `true`), and `category` (default `apiwatch`). With `sarif: true`, the action writes and uploads a SARIF report, then preserves the check's exit code. The caller needs `security-events: write`; the upload never changes the result. Do not put `--json` in `check-args` when SARIF is enabled. Keep the database across runs with `actions/cache` as shown in the previous section — the action does not manage caching itself. The repository's own pipeline tracks `examples/spec.yaml` through the action in a Dogfood workflow.

### Scheduled check with a cached baseline

```yaml
name: API contract watch

on:
  schedule:
    - cron: "0 8 * * *"
  workflow_dispatch:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2

      - name: Restore API Watch database
        uses: actions/cache/restore@v4
        with:
          path: ~/.apiwatch
          key: apiwatch-${{ github.run_id }}
          restore-keys: apiwatch-

      - run: cargo install --path . --locked

      - name: Check APIs
        env:
          APIWATCH_DB: ~/.apiwatch/apiwatch.db
          VENDOR_TOKEN: ${{ secrets.VENDOR_TOKEN }}
        run: apiwatch check --fail-on-breaking

      - name: Save API Watch database
        if: always()
        uses: actions/cache/save@v4
        with:
          path: ~/.apiwatch
          key: apiwatch-${{ github.run_id }}
```

Add targets once on the runner (or in a setup step), then let the scheduled job report drift. `NO_COLOR=1` disables ANSI output; `TERM=dumb` and `CLICOLOR=0` do the same.

For local monitoring, a cron entry is enough:

```cron
0 */6 * * * /usr/local/bin/apiwatch check --fail-on-breaking >> /tmp/apiwatch.log 2>&1
```

## SARIF output

`apiwatch check --sarif` emits [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html) for GitHub code scanning and other SARIF consumers:

```bash
apiwatch check --sarif > apiwatch.sarif
```

| Rule | Level | Meaning |
| --- | --- | --- |
| `apiwatch/breaking` | `error` | A contract change that can break existing clients |
| `apiwatch/compatible` | `note` | An additive or relaxed change |
| `apiwatch/check-error` | `error` | The target could not be fetched or compared |

Each result carries the target name, change label, and detail. When the tracked source is a local spec file, the result points at that file relative to the repository root, so pull requests get file annotations. A remote spec keeps its URL as the artifact location: the alert still names the changed contract, but there is no repository line to annotate.

```yaml
permissions:
  contents: read
  security-events: write

steps:
  - uses: actions/checkout@v4
  - uses: dtolnay/rust-toolchain@stable
  - run: cargo install --git https://github.com/hix-pro/api-watch --locked
  - run: apiwatch check --sarif --fail-on-breaking > apiwatch.sarif
  - uses: github/codeql-action/upload-sarif@v3
    if: always()
    with:
      sarif_file: apiwatch.sarif
      category: apiwatch
```

The composite action above does this automatically when `sarif: true`. `--sarif` and `--json` are mutually exclusive.

## JSON output

```bash
apiwatch check --json
apiwatch verify orders ./openapi.yaml --json
apiwatch list --json
apiwatch history petstore --json
```

`check --json` returns:

```json
{
  "targets": [
    {
      "name": "orders",
      "url": "https://example.com/openapi.json",
      "status": "changed",
      "checkedAt": "2026-10-26T17:39:21Z",
      "format": "OpenAPI",
      "title": "Orders API 1.1.0",
      "operations": 3,
      "facts": 17,
      "breaking": 1,
      "compatible": 1,
      "httpStatus": 200,
      "message": null,
      "unresolvedRefs": 0,
      "changes": [
        {
          "kind": "breaking",
          "area": "response",
          "label": "GET /orders · 200 application/json[].total",
          "detail": "response property removed"
        }
      ]
    }
  ],
  "summary": {
    "checked": 1,
    "changed": 1,
    "breaking": 1,
    "compatible": 1,
    "errors": 0
  }
}
```

## Storage and security

- Default location: `~/.apiwatch/apiwatch.db`. The directory is created `0700` and the database file is set to `0600` on Unix.
- Snapshots contain the **full text** of every changed spec. Treat the database as sensitive; do not commit it.
- Request headers, including bearer tokens, are stored in plaintext so repeated checks can authenticate. Use a least-privilege token and rotate it. Removing a target deletes its data.
- Custom headers are sent only to the target's origin. If a target redirects to a different scheme, host, or port while custom headers are configured, the check fails instead of forwarding them.
- API Watch only makes network requests to the URLs you add. There is no telemetry.
- WAL mode is enabled, so SQLite may create `-wal` and `-shm` files next to the database.

## Limitations

- **OpenAPI 3.x and Swagger 2.0 only** (plus generic JSON). External `$ref`s are not fetched; unresolved refs are counted and shown as a warning.
- **Not a full JSON Schema prover.** `allOf`, `oneOf`, and `anyOf` are traversed, and `allOf` required fields are merged across branches, but composition is not fully proven. Unusual schemas can produce approximate results.
- **Static headers only.** No OAuth2 client-credentials flow, mTLS, or signed requests.
- **No notifications built in.** Use cron/CI and route stdout, or consume `--json`.
- **No retries.** One attempt per target per run; failed checks are recorded and return code `1`.
- **Specs are the source of truth.** A server that changes behavior without changing its spec will not be detected.
- **Snapshot growth.** Changed documents are stored in full. A frequently changing 10 MB spec creates a large database; remove and re-add the target to reset it.

## Roadmap

- External `$ref` resolution and a config file with env-backed headers
- Markdown diffs for pull-request comments
- Scheduled GitHub Action with artifact storage
- Notifications (Slack, webhook, email)
- GraphQL SDL and protobuf schema support
- Per-target intervals and response-time checks

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

The test suite covers OpenAPI, Swagger 2, and YAML flattening, `$ref` resolution, all media types, response headers, `allOf` required merging, request/response enum direction, type and required-field changes, diff cascade suppression, generic JSON diffs, header validation, local file handling, redirect policy, and CLI exit codes.

## Related tools

| Tool | What it does |
| --- | --- |
| [EnvAudit](https://github.com/hix-pro/env-audit) | find missing, undocumented, and unused environment variables |
| [Migration Guard](https://github.com/hix-pro/migration-guard) | statically catch dangerous SQL migrations before they run |
| [RepoRace](https://github.com/hix-pro/reporace) | see which GitHub repositories are accelerating |
| [TypeShift](https://github.com/hix-pro/typeshift) | decide the semver bump from a TypeScript declaration diff |
| [ActionLens](https://github.com/hix-pro/action-lens) | see what a GitHub Actions workflow really does, before it hits main |
| [ComposeMap](https://github.com/hix-pro/compose-map) | understand your Docker Compose stack and its production risks at a glance |
| [OpenCode Telegram](https://github.com/hix-pro/opencode-telegram) | control a local OpenCode server from Telegram |
---

_Part of the **[hix tools](https://github.com/hix-pro#readme)** suite — local-first, no backend, CI-ready._

**Deep dive:** [Breaking API changes you didn't know you shipped](https://hix-pro.github.io/blog/posts/03-api-watch-breaking-api-changes.html)

## License

MIT. See [LICENSE](LICENSE).
