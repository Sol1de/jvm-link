# jvm-link

An independent Rust library for accessing a JVM **already running in the current
process**, on Windows x64. The host always owns the JVM.

Java 21 is the initial target. The architecture allows future support for other
versions, including Java 8 and 17; those versions are not currently supported.

## Current status

This is a design prototype with a compilable public contract, not a working JVM
connection library. The temporary `jvm-link-contract` package is not publishable.
Its trait describes future inherent methods on an opaque `Connection` and has no
implementation. Internal Windows JVM discovery and thread attachment scopes are
implemented and tested. Exception normalization, compatibility validation,
connection lifecycle and publication remain future work.

Crate configuration values live directly in the root `config.toml`, without
sections; a single `Config` struct in `src/config.rs`
loads and validates them once. The file is embedded at compilation, so no runtime
configuration file is required. Editing it requires rebuilding the crate.

- [Specifications](spec/README.md): scope, architecture, API, ownership and lifecycle.
- [Compilable contract and doctests](src/lib.rs).
- [Implementation guide](AGENTS.md).

## Checks

Run from the project root on Windows x64:

```powershell
cargo check --locked
cargo test --lib --locked
cargo test --doc --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

These checks do not start a JVM. The doctests compile a valid consumer and reject
escaping or transferring local environments and references. Runtime integration
tests must use dedicated technical Java 21 hosts with `-Xcheck:jni`.

The dedicated discovery host compiles the private lookup module unchanged into a
test-only DLL, compares the discovered pointer with JNI `GetJavaVM`, and confirms
that Java continues executing after repeated discovery. Run it with a Java 21
JDK (use its explicit path rather than the Java version on `PATH`):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tests/java21-host/run.ps1 -JavaHome 'C:\path\to\jdk-21'
```

This uses a process-local script execution policy and does not change system
settings. Java starts and owns the technical JVM; the library never creates or
destroys it. The same host checks borrowed Java-thread scopes and native daemon
scopes, nested frames, early errors and Rust unwinding. The outer local reference
survives inner scopes, the Java thread remains attached, and the native thread is
detached after each outer scope. Expected Rust panics are caught inside the
adapter; their diagnostic output is normal. Injected detach failures and
owner-thread retries are covered by unit tests, not by the real JVM.
