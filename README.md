# API Watch

**Catch breaking changes in the APIs you depend on — before they reach production.**

[![CI](https://github.com/hix-pro/api-watch/actions/workflows/ci.yml/badge.svg)](https://github.com/hix-pro/api-watch/actions/workflows/ci.yml)
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

Requires **Rust 1.85+**. SQLite is bundled; there are no system dependencies.

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
| `apiwatch add <source>` | Track a URL or local path. `--name` overrides the derived name; `-H/--header` adds a request header. |
| `apiwatch check [name]` | Fetch and compare. No name checks every target. `--json`, `--fail-on-breaking`. |
| `apiwatch list` | Show targets and their last result. Alias: `ls`. `--json`. |
| `apiwatch history <name>` | Show past checks. `--limit` (default 10, max 1000), `--json`. |
| `apiwatch remove <name>` | Stop tracking and delete snapshots, checks, and changes. Alias: `rm`. |

Global option: `--db <path>` selects the database (default `~/.apiwatch/apiwatch.db`, env `APIWATCH_DB`).

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | All targets checked. Includes compatible-only changes, and breaking changes **without** `--fail-on-breaking`. |
| `1` | Fetch, parse, or usage error (also recorded in history). |
| `2` | Breaking changes found **and** `--fail-on-breaking` was set. |

This distinction matters: a tool that always exits non-zero on changes cannot be used for monitoring without also failing on safe additions.

## What counts as breaking

API Watch compares a flattened set of contract facts and applies different rules to requests and responses.

| Change | Classification |
| --- | --- |
| Endpoint removed | **Breaking** |
| Endpoint added | Compatible |
| Request parameter or body property removed | **Breaking** |
| New required request parameter or property | **Breaking** |
| New optional request parameter or property | Compatible |
| Request field becomes required | **Breaking** |
| Request field becomes optional | Compatible |
| Response property removed | **Breaking** |
| Response property added | Compatible |
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

## How it works

1. **Fetch** the URL or read the local file. Previous ETags are sent as `If-None-Match`; a `304` records an unchanged check without downloading the spec. Responses are capped at **25 MB** with a **30 second** timeout.
2. **Parse** JSON or YAML and detect OpenAPI/Swagger. Local `$ref`s are resolved (up to 32 levels).
3. **Flatten** the document into facts keyed by a stable label, for example `POST /pets · body application/json.name` or `GET /pets · 200 application/json[].status`.
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

The key detail: CI needs a persistent baseline. Keep the SQLite database in a cache, a private artifact store, or a self-hosted persistent volume. If the cache is cold, API Watch records a baseline and exits `0`.

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

## JSON output

```bash
apiwatch check --json
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
- API Watch only makes network requests to the URLs you add. There is no telemetry.
- WAL mode is enabled, so SQLite may create `-wal` and `-shm` files next to the database.

## Limitations

- **OpenAPI 3.x and Swagger 2.0 only** (plus generic JSON). External `$ref`s are not fetched; unresolved refs are counted and shown as a warning.
- **Not a full JSON Schema prover.** `allOf`, `oneOf`, and `anyOf` are traversed, but composition is not logically merged. Unusual schemas can produce approximate results.
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

The test suite covers OpenAPI and YAML flattening, `$ref` resolution, request/response enum direction, type and required-field changes, generic JSON diffs, header validation, and local file fetching.

## License

MIT. See [LICENSE](LICENSE).
