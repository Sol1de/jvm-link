# jvm-link

An independent Rust library for accessing a JVM **already running in the current
process**, on Windows x64. The host always owns the JVM.

Java 21 is the initial target. The architecture allows future support for other
versions, including Java 8 and 17; those versions are not currently supported.

## Current status

The non-publishable `jvm-link-contract` prototype now exposes an opaque
`Connection`: unsafe discovery/admission of an existing Java 21 JVM, scoped
operations and explicit retryable shutdown. It implements attachment ownership,
reference cleanup and exception normalization. Publication remains future work.

Call `unsafe { Connection::connect_existing() }` only while the host guarantees
JVM/module liveness through every attachment and exported global, including
globals in errors. Use `with_env` for operations and explicitly retry `close`
before owner threads exit. A `ConnectionFailed` error retains the closing
connection if constructor cleanup is outstanding; retry its `close` too.

Crate configuration values live directly in the root `config.toml`, without
sections; a single `Config` struct in `src/config.rs`
loads and validates them once. The file is embedded at compilation, so no runtime
configuration file is required. Editing it requires rebuilding the crate.

- [Specifications](spec/README.md): scope, architecture, API, ownership and lifecycle.
- [Compilable contract and doctests](src/lib.rs).
- [Implementation guide](AGENTS.md).

## Checks

Optional JVMTI sessions are enabled with `--features jvmti`. Reserve a session
with `connection.jvmti_session()`, open it with explicit `JvmtiCapabilities`, and
explicitly retry its `close()` on errors before closing the connection. Opening
is separate from reservation so failures cannot hide an environment needing
cleanup. No callbacks or event notifications are installed. Failed Drop cleanup
retains the same environment and error in the VM's owner-thread registry. Retry
`Connection::close()` on that thread, even through a later connection to the same
VM. A different thread reports `CleanupPending`. See the specifications for ownership.

Run the checks below both with and without `--features jvmti` (formatting is shared).
The technical host accepts `-Jvmti` to exercise sessions on borrowed Java threads
and detached native threads, capability requests, explicit/repeated close and Drop.
Disposal failures and pointer validation use injected unit tests.

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

The host also exercises shutdown inside an operation, rejection of new scopes
while closing, preservation of an active native scope and its local references,
and repeated/concurrent close. Unit tests inject detach failures, including
unwinding and destructor cleanup, and verify that another thread cannot detach
the owner's attachment. Explicit close must finish on worker threads before they
exit. Drop only attempts local cleanup; it performs no diagnostic I/O. Unreported
cleanup failures survive successful detach and are returned by a later owner-thread
close, even through another connection to the same VM. After observing such an
error, call close again to finish the state transition.

The host uses the exported constructor and lifecycle API on borrowed and detached
threads. It also changes/removes `java.specification.version` to check admission
and rejection paths; these are property tests on Java 21, not real Java 8/17 tests.

The host also checks local-reference release on success, errors and unwinding,
exported globals and throwable globals surviving frame exit and becoming
collectible after release, and pending exceptions preserved by identity on entry.
Operation exceptions are captured even after `Ok`, with their original operation
errors retained. A test-only JNI table override injects failed `NewGlobalRef`
allocation both with and without a pending `OutOfMemoryError`, without exhausting
the host heap. It also checks failed `PushLocalFrame` allocation: the callback
never runs, the frame error and throwable are retained, and no exception remains
pending. Capture failures preserve the typed JNI cause. Exception capture avoids
Java diagnostic calls; see
[lifecycle](spec/lifecycle.md) for error and destruction semantics.
