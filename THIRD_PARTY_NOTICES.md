# Third-party notices

apiwatch is MIT licensed. It links the following Rust crates. Exact versions
are recorded in [`Cargo.lock`](Cargo.lock); each crate is used under the license
terms published by its authors.

| Crate | License | Project |
| --- | --- | --- |
| `anyhow` | MIT OR Apache-2.0 | https://github.com/dtolnay/anyhow |
| `chrono` | MIT OR Apache-2.0 | https://github.com/chronotope/chrono |
| `clap` | MIT OR Apache-2.0 | https://github.com/clap-rs/clap |
| `dirs` | MIT OR Apache-2.0 | https://github.com/dirs-dev/dirs-rs |
| `reqwest` | MIT OR Apache-2.0 | https://github.com/seanmonstar/reqwest |
| `rusqlite` | MIT OR Apache-2.0 | https://github.com/rusqlite/rusqlite |
| `serde` | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde_json` | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| `serde_yaml_ng` | MIT | https://github.com/acatton/serde-yaml-ng |

SQLite is compiled into the binary in *bundled* mode (`rusqlite` with the
`bundled` feature). The SQLite C library itself is in the public domain
(https://www.sqlite.org/copyright.html). TLS is provided by `rustls` and its
supporting crates rather than a system OpenSSL.

Rust, Cargo, and the standard library are distributed under their own licenses.
They are build tooling and a runtime dependency, not vendored source.
