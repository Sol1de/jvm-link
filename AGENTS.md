# Project overview

jvm-link is an independent Rust library for borrowing an existing in-process JVM
on Windows x64. Java 21 is the initial target; Java 8 and 17 are future targets.
The current package, `jvm-link-contract`, is a non-publishable design prototype,
not a working JVM connection library. Internal Windows discovery and attachment scopes are implemented.
See [README.md](README.md).

Before implementation, read the [specification index](spec/README.md) and its
linked documents, then [src/lib.rs](src/lib.rs) for the compilable public contract.
Implement the requested ticket only; use Kaneo for ticket scope and dependencies.

# Setup & commands

Use Rust/Cargo on Windows x64. Run these commands from the project root:

```powershell
cargo check --locked
cargo test --lib --locked
cargo test --doc --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
```

These commands check the current prototype without starting a JVM.

# Code style and architecture

- Reuse `jni` 0.22.4 with default features disabled and without `invocation`.
  Reuse its environments, references, frames and errors; do not build another JNI SDK.
- Keep JVM lookup in the internal `src/vm_lookup.rs` module.
- Centralize Java/JNI compatibility decisions internally. Keep connection,
  ownership and scope logic shared across supported Java versions.
- Distinguish Java runtime, JNI interface and Rust binding versions. Do not
  assume `GetVersion` identifies Java or silently assume Java 21 on detection failure.
- Initially admit only Java 21. Add another supported version only with a
  compatibility review and dedicated real-JVM tests.
- When implementing the runtime, replace `ConnectionContract` with inherent
  methods on an opaque `Connection`; transfer its compile-time checks to the real API.
- Keep detailed decisions in `spec/`. If an implementation changes the
  public contract or a design decision, update the relevant specification and
  signatures/tests in the same change. Keep this file a short implementation guide.

# Testing

- For Rust changes, run the applicable checks above and tests for the changed behavior.
- Preserve compilation failures for escaping a local environment/reference and
  transferring either to another thread through the safe API.
- A compiling signature is not proof of runtime cleanup or JVM ownership behavior.
  Report real-JVM checks separately from compile-time checks.
- Runtime integration tests must use dedicated technical Java 21 hosts, with
  `-Xcheck:jni`. No application client, injector or demo host belongs in this crate.

# Safety and lifecycle invariants

- Never create or destroy the host JVM. Discover it through the already-loaded
  `jvm.dll`; validate JNI return codes and pointers before conversion or use.
- Keep the host-liveness obligation explicit at the unsafe connection boundary,
  including the lifetime of exported globals and errors containing globals.
- Never store or share a thread-local environment across threads. Keep local
  references inside their scoped frame; allow only owned values/globals to escape.
- Borrow pre-existing thread attachments without detaching them. Attach detached
  threads as daemons; detach only attachments owned by this crate on their owner thread.
- Preserve nested scope ownership and cleanup on errors and unwinding. Do not hold
  a connection-wide lock while executing Java or user callbacks.
- Preserve pre-existing Java exceptions. Follow the design's structured error
  policy for exceptions produced by operations and for simultaneous cleanup failures.
- Make connection shutdown explicit, idempotent and retryable. Reject new scopes while closing,
  let admitted scopes finish, and retain ownership when cleanup fails.
- Keep JVMTI optional and separate. Do not add application-specific dependencies,
  mappings or overlays.
