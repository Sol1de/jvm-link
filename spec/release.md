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
4. Configure the repository secret described below before pushing the release
   commit to `main`. The workflow publishes after the full validation matrix
   succeeds. A merged pull request also produces this push event.

### Automatic publication from main

The `publish` job in `.github/workflows/ci.yml` runs only for pushes to `main`
in `Sol1de/jvm-link`, after all four Windows validation jobs pass. It checks the
manifest version against crates.io: an existing version (including a yanked one)
is skipped, a missing version is published, and registry/network errors fail the
job. Publication jobs use `queue: max` and `cancel-in-progress: false`: one runs
at a time and up to 100 can wait without replacing earlier pending jobs. GitHub
cancels additional arrivals if that queue is full. Queue order follows admission
after validation, not commit order.
After publication succeeds, the same job creates the lightweight tag
`v<package.version>` on the published commit. Only this job has `contents: write`;
validation jobs retain `contents: read`. Tag creation uses the built-in
`GITHUB_TOKEN`; no additional secret is required. Repository tag rules must allow
GitHub Actions to create these tags.

An existing tag is always left unchanged, even if it points elsewhere. When a
version already exists but its tag is missing, the job recovers the commit from
the published archive's `.cargo_vcs_info.json` and creates the tag there. This
also avoids tagging a later commit that kept the same version. Missing/dirty
VCS metadata or GitHub errors fail the job rather than guessing a commit. The
GitHub API only creates references; the workflow never updates or forces a tag.
Pull request runs, other branches and manual workflow dispatch only validate.
To retry a failed publication or tag creation, rerun the failed jobs on the
original push run. If the upload already succeeded, it is skipped and the missing
tag is created. If publication fails, no tag is created.

`.github/tests/release-workflow.ps1` runs in the debug/JNI validation job, so its
failure blocks publication. It tests the actual workflow gate for 18 event/ref/
repository combinations and calls the release scripts in `.github/scripts/`
directly with mocked HTTP and a real archive. It checks existing/missing versions
and registry errors, the workflow commit for a new publication, recovery of
the original published commit when a later push keeps the version, and retention
of an existing tag on another commit. Run it locally with
`powershell -NoProfile -ExecutionPolicy Bypass -File .github/tests/release-workflow.ps1`;
it needs no tokens or network access.

The workflow delegates Cargo validation to `.github/scripts/validate.ps1` and
technical host validation to `.github/scripts/java21-host.ps1`, passing the matrix
profile and feature selection. Version lookup and tag creation live in
`check-published-version.ps1` and `create-release-tag.ps1` in the same directory.
Triggers, conditions, permissions, secrets, job dependencies and the publish
command remain in the workflow.

Repository owner setup:

1. Sign in to crates.io and verify the account email. Confirm that `jvm-link` is
   available for its first publication, or that the account owns the existing crate.
2. Create an API token at <https://crates.io/settings/tokens>, restricted to
   `jvm-link`, with permission to publish new versions (`publish-update`) and,
   for the first publication, create the crate (`publish-new`). Set an expiration
   and replace the token before it expires. After the first release, remove the
   crate-creation permission when replacing it.
3. In GitHub repository Settings > Secrets and variables > Actions, create a
   repository secret named `CARGO_REGISTRY_TOKEN` containing that token. The
   workflow exposes it only to the publish step. Do not commit or log the token.
4. For each release, update `package.version` in `Cargo.toml` and refresh the
   root `Cargo.lock` and the technical adapter's `tests/java21-host/Cargo.lock`.
   Commit these changes and merge/push to `main`. The workflow creates the tag.

A push with an unpublished manifest version publishes it automatically; a push
with an already published version skips the upload and ensures its tag exists.
There is no automatic version bump. Published versions cannot be overwritten. Protect
`main` with required reviews and validation checks to control release access.

Token scoping is documented in the [crates.io token announcement](https://blog.rust-lang.org/2023/06/23/improved-api-tokens-for-crates-io/).
See the [Cargo publication guide](https://doc.rust-lang.org/cargo/reference/publishing.html)
for account setup and versioning. Trusted Publishing is a possible later
replacement for the stored token; it is not configured by this workflow.

During local preparation, `cargo package --locked --offline --allow-dirty`
can verify an uncommitted archive. CI requires a clean checkout. Packaging proves
the archive builds; it does not establish registry ownership or upload permission.
