# Release validation

## Supported release

The initial package is `jvm-link` 0.1.0 under the MIT licence, imported as `jvm_link`, and targets Windows x64
with Java 21 only. `jni` 0.22.4 is pinned without default features or invocation.
Consumers of the prototype must change their dependency from `jvm-link-contract`
to `jvm-link` and Rust imports from `jvm_link_contract` to `jvm_link`.
The default build is JNI-only; `jvmti` is optional and validated alongside it.
No application, injector, Minecraft dependency or other catalogue crate is used.

## Validation matrix

`.github/workflows/ci.yml` runs on pushes, pull requests and manual dispatch.
Each of its four Windows jobs combines debug/release with JNI-only/JVMTI:

- Check formatting for the library and the dedicated host adapter.
- Run Cargo check, Clippy with warnings denied and unit tests with injected JNI
  and JVMTI failures, including owner-thread cleanup retries.
- Run doctests, including compilation failures for escaping environments/local
  references or transferring them to another thread.
- Build documentation with warnings denied.
- Build and verify the source package in isolation.
- Lint the technical adapter and run Java 21 with `-Xcheck:jni`.

`tests/java21-host/run.ps1` accepts `-JavaHome`, `-Jvmti` and `-Release`.
The Java process creates and owns its VM; the adapter only borrows it. In addition
to exception, reference, nesting and shutdown checks, the host runs 32 public API
connection cycles on both a Java thread and a native thread. Each cycle executes
eight nested operations, closes twice and verifies that new operations fail.
Attachment status is checked after construction, each operation and destruction.

Mocks establish behavior under injected cleanup/allocation failures; real-host
tests establish behavior on Java 21. Neither compilation nor a mock proves actual
VM cleanup. The host's property mutation tests do not establish Java 8/17 support.
The safety boundary and lifetime obligations are documented in crate-level API
documentation and in [ownership](ownership.md) and [lifecycle](lifecycle.md).

Local validation on 2026-11-05 used Windows x64, Rust 1.99.1 and Temurin
21.0.12.101. All four configurations passed check, Clippy, documentation,
unit tests (30 JNI-only, 40 with JVMTI), doctests and the checked real-JVM host.
Formatting and package verification passed. Both JNI-only and JVMTI
`cargo publish --dry-run --locked --allow-dirty` completed without uploading.
The GitHub-hosted workflow has not been executed during this local preparation.

## Publication procedure

The manifest declares the repository, README, documentation URL, keywords,
category and Windows x64 docs.rs target. Explicit root-anchored package paths
include the implementation, embedded `config.toml`, README, licence and specs.
Technical host fixtures, build outputs and local tool settings are excluded.

Before the first publication:

1. Confirm the release metadata: version `0.1.0`, SPDX identifier `MIT`, included
   `LICENSE` text and publication enabled in `Cargo.toml`.
2. Check the `jvm-link` name on crates.io again. A successful empty
   `cargo search jvm-link --limit 10` was observed on 2026-11-05; this does not
   reserve the name or establish publisher permissions.
3. Run the full CI matrix on the exact release commit. Inspect
   `cargo package --locked --list`, then run `cargo publish --dry-run --locked`
   with and without `--features jvmti`. Confirm the generated documentation.
4. Publish manually with an authorised crates.io account, then tag the released
   commit. CI has no publication credentials or automatic publish step.

During local preparation, `cargo package --locked --offline --allow-dirty`
can verify an uncommitted archive. CI requires a clean checkout. Packaging proves
the archive builds; it does not establish registry ownership or upload permission.
