# Exceptions, cleanup and shutdown

## Java exceptions and Rust unwinding

- If a Java exception is pending on entry, return `PendingJavaException` without
  running the closure or clearing that exception. The host owns its handling.
- Capture exceptions produced by an operation using the binding's references,
  clear their pending state and return `JavaException { throwable, operation }`.
  The throwable is a binding `Global<JThrowable>`; `operation` retains any error
  returned by the closure, including its original JNI cause. Check on exit even
  if the closure returns `Ok`. Already-caught binding errors, including
  `CaughtJavaException`, pass through unchanged when no exception is pending.
- If both an operation and detachment fail, `Cleanup` retains both errors and the
  owning thread identifier.
- If throwable promotion fails, return `ExceptionCapture { source, operation }`
  with the typed JNI capture cause and original operation error. Clear any
  secondary pending exception before cleanup; no throwable is claimed recovered.
  Test allocation failure in particular.
- A Rust panic continues unwinding after cleanup; never convert it into success.
  Retain any cleanup failure during unwinding in shared VM ownership state for `close()`
  to report.

Reuse binding capture/rethrow facilities without silently clearing an exception
owned by the host. See the [binding errors](https://docs.rs/jni/0.22.4/jni/errors/enum.Error.html).

The internal scope uses `exception_occurred`, `exception_clear` and
`new_global_ref`. It explicitly releases the temporary throwable local, including
when frame allocation failed. Unlike `jni` 0.22.4's `exception_catch`, this keeps
promotion errors typed rather than substituting an error containing a null global.
It does not call Java for diagnostic strings or stack traces during capture;
consumers can inspect or rethrow the retained throwable in a later scope. The
throwable itself provides the Java failure context. During Rust unwinding, the
scope clears operation exceptions and drops their temporary globals before frame
cleanup, then resumes the original panic. No operation callback runs on a pending
entry exception. Frame allocation errors are normalized before detachment too.

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

The public `Connection` implements these transitions and validates Java 21 during
construction. `Drop` attempts only owner-thread cleanup; it does not wait for
active scopes and performs no diagnostic I/O. TLS retains a failed attachment.
The shared owner registry retains unreported unwind/Drop cleanup causes
even after a successful detach. Owner-thread `close` retries cleanup and reports
those causes; a simultaneous new cleanup error wraps the old cause in `operation`.
After observation, another close completes the transition to Closed. Drop never
consumes an unreported error. A later connection to that VM can retry/report on
the owner thread; an exited owner cannot be repaired remotely. Because ownership
is shared per VM, `CleanupPending` may also
report an attachment still active through another connection, including the
calling thread; it is never detached while its scope depth is nonzero.

## Optional JVMTI disposal

Session close retains its exact owned environment until `DisposeEnvironment`
returns success. Capability errors also retain ownership. A successful disposal
clears the pointer before scope cleanup, so a failed detach cannot cause a second
disposal of an invalid environment. Keep the session and retry close on errors;
the connection stays busy until session close succeeds or the session is dropped.
Drop makes one non-panicking close attempt. On failure it transfers the exact
remaining environment and error to the existing per-VM, owner-thread registry,
before releasing its admission. Connection close retries that queue on its owner
thread without holding the registry lock across JNI/JVMTI. Other threads report
`CleanupPending`, including while a retry is in progress. A successful retry
still reports each retained error once; call close again to complete shutdown.
Connection Drop may retry resources but never consumes their unreported errors.
The queue survives connection destruction and is available through a later
connection to the same VM. No thread-local JNI environment is stored or shared.
Explicit close must finish before the owner thread exits; a departed owner cannot
be repaired remotely. Host liveness must cover retained environments and errors.
No connection or session cleanup destroys the JVM.

## Required runtime checks

Test borrowed Java threads and native daemon attachments, nested scopes, early
returns, unwinding and local/global reference release. Verify that host-owned
attachments and pre-existing exceptions survive operations unchanged.

Exercise Java call failure, repeated/concurrent shutdown, shutdown inside a
closure, active-operation preservation, detach failure followed by retry on the
owning thread, and simultaneous operation/cleanup failures. Real-host tests must
use Java 21 with `-Xcheck:jni`; compile-time checks alone do not establish these
guarantees.
