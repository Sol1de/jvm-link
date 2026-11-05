# Specifications

These documents define the intended jvm-link behavior. The current Rust package
verifies the public API by compilation and implements Windows JVM discovery,
thread attachment scopes, Java 21 admission and a public connection lifecycle.

## Scope

Build an independent Rust library for borrowing an existing in-process JVM on
Windows x64. Java 21 is the initial target, with future Java versions accommodated
by the architecture. No application-specific dependencies, Minecraft integration,
mappings, overlays, injectors or demonstration clients belong in this crate.
JVMTI is optional and separate from the required JNI core.

## Reading order

| Document | Responsibility |
| --- | --- |
| [Architecture](architecture.md) | Module boundaries, JNI binding choice, compatibility and platform lookup. |
| [Public API](public-api.md) | Connection operations, error categories and the compilable contract. |
| [Ownership](ownership.md) | Host lifetime, thread attachments, local frames and global references. |
| [Lifecycle](lifecycle.md) | Exception handling, cleanup, shutdown and retries. |
| [Release validation](release.md) | Validation matrix, package contents and publication procedure. |

[src/lib.rs](../src/lib.rs) is the authoritative compilable signature definition.
The API specification explains it without maintaining a second full code copy.
Keep specifications, signatures and tests aligned when changing a decision.

## Validation and delivery

Use the [root README](../README.md#checks) for current check commands. Compilation
proves signature compatibility with the selected binding and rejects certain
lifetime/thread violations. It does not prove runtime ownership or cleanup.

Runtime tests must cover borrowed and newly attached threads, nested scopes,
errors and unwinding, reference release, Java exceptions, concurrent shutdown and
failed cleanup followed by retry. Use dedicated Java 21 hosts with `-Xcheck:jni`.
Each future supported Java version needs its own real-host test coverage.

CI and publication work must include formatting, Clippy, debug/release tests,
documentation and package validation. If JVMTI is shipped, its tests must pass
alongside a JNI-only build with that feature disabled.

Ticket sequencing and dependencies remain in Kaneo. These specifications preserve
the technical decisions independently of individual ticket names.
