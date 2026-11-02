# Exceptions, cleanup and shutdown

## Java exceptions and Rust unwinding

- If a Java exception is pending on entry, return `PendingJavaException` without
  running the closure or clearing that exception. The host owns its handling.
- Capture exceptions produced by an operation using the binding's mechanisms,
  clear their pending state and return a structured error. A `CaughtJavaException`
  carries a global throwable and context. Check on exit even if the closure returns
  `Ok`.
- If both an operation and detachment fail, `Cleanup` retains both errors and the
  owning thread identifier.
- If exception capture itself fails, preserve the primary JNI error without
  pretending to have recovered a throwable. Test allocation failure in particular.
- A Rust panic continues unwinding after cleanup; never convert it into success.
  Retain any cleanup failure during unwinding in connection state for `close()`
  to report.

Reuse binding capture/rethrow facilities without silently clearing an exception
owned by the host. See the [binding errors](https://docs.rs/jni/0.22.4/jni/errors/enum.Error.html).

## Shutdown state

The connection states are `Open`, `Closing` and `Closed`. Transition from `Open`
to `Closing` atomically with scope admission. Reject new scopes after that
transition. Previously admitted operations finish normally: shutdown does not
interrupt them or release resources they still use.

`close(&self)` does not wait for active scopes. If any remain, return
`Busy { active_scopes }` and retain `Closing`; a later call completes shutdown.
This also applies when called inside an operation, without deadlocking. Successful
shutdown transitions to `Closed`; subsequent calls return `Ok(())`.

## Retryable cleanup

A failed detachment remains owned by the thread that created the attachment.
Calling `close()` on that thread retries its cleanup. A different thread receives
`CleanupPending { owners }`; it never detaches a remote thread. Workers must
coordinate cleanup before exiting rather than expecting the main thread to repair
their pending detachments. Thread identifiers are diagnostics, not permission to
act on another thread's behalf.

`Drop` attempts local cleanup without blocking or panicking. It cannot guarantee
remote cleanup or return an error: retain outstanding cleanup in the owning
thread's state and report failure without claiming successful shutdown. A residual
leak is preferable to releasing a resource still in use. Explicit `close()` before
workers exit is the contractual cleanup path.

Exported globals are neither invalidated by shutdown nor collected in a hidden
registry. Consumers release them through the binding. The host must remain alive
until they are released. No jvm-link shutdown or destructor calls `DestroyJavaVM`.

## Required runtime checks

Test borrowed Java threads and native daemon attachments, nested scopes, early
returns, unwinding and local/global reference release. Verify that host-owned
attachments and pre-existing exceptions survive operations unchanged.

Exercise Java call failure, repeated/concurrent shutdown, shutdown inside a
closure, active-operation preservation, detach failure followed by retry on the
owning thread, and simultaneous operation/cleanup failures. Real-host tests must
use Java 21 with `-Xcheck:jni`; compile-time checks alone do not establish these
guarantees.
