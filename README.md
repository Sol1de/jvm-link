# jvm-link

An independent Rust library for accessing a JVM **already running in the current
process**, on Windows x64. The host always owns the JVM.

Java 21 is the initial target. The architecture allows future support for other
versions, including Java 8 and 17; those versions are not currently supported.

## Current status

This is a design prototype with a compilable public contract, not a working JVM
connection library. The temporary `jvm-link-contract` package is not publishable.
Its trait describes future inherent methods on an opaque `Connection` and has no
implementation. Runtime behavior and publication metadata remain future work.

- [Specifications](spec/README.md): scope, architecture, API, ownership and lifecycle.
- [Compilable contract and doctests](src/lib.rs).
- [Implementation guide](AGENTS.md).

## Checks

Run from the project root on Windows x64:

```powershell
cargo check --locked
cargo test --doc --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

These checks do not start a JVM. The doctests compile a valid consumer and reject
escaping or transferring local environments and references. Runtime integration
tests must use dedicated technical Java 21 hosts with `-Xcheck:jni`.
