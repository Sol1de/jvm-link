# jvm-link

An independent Rust library for accessing a JVM **already running in the current
process**, on Windows x64. The host always owns the JVM.

Java 21 is the initial target. The architecture allows future support for other
versions, including Java 8 and 17; those versions are not currently supported.

## Current status

This is a design prototype with a compilable public contract, not a working JVM
connection library. The temporary `jvm-link-contract` package is not publishable.
Its trait describes future inherent methods on an opaque `Connection` and has no
implementation. Internal Windows JVM discovery is implemented and tested; scopes,
compatibility validation, connection lifecycle and publication remain future work.

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
destroys it. This test does not validate attachment, scopes or cleanup.
