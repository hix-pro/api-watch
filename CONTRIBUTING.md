# Contributing to API Watch

Thanks for helping improve API Watch! This guide covers what you need to work on
the codebase and get a PR merged.

## Development setup

You need a Rust toolchain. The repository pins `stable` in `rust-toolchain.toml`,
so a current rustup install matches CI:

```bash
git clone https://github.com/hix-pro/api-watch.git
cd api-watch
cargo build
cargo test
```

## Checks before you push

CI runs the same commands, so run them locally first:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --locked
```

Ground rules:

- No `unwrap()` or `expect()` on user-facing paths — return friendly errors and
  use the documented exit codes.
- New behavior needs tests; bug fixes need a test that fails before the fix.
- Update the README when you add or change a flag, rule, or output format.

## Submitting changes

1. Open an issue first for anything bigger than a small fix, so the approach can
   be agreed on before you invest time.
2. Keep PRs focused — one behavior change per PR.
3. Use conventional commit messages (`feat:`, `fix:`, `docs:`, `ci:`, `chore:`);
   they feed the changelog.
4. Fill out the PR template and make sure CI is green.

## Reporting bugs

Open a bug report with steps to reproduce, the version or commit you are on, and
your platform. Minimal input files that trigger the bug are especially helpful.
For security issues, see [SECURITY.md](SECURITY.md) — please do not open public
issues for those.
