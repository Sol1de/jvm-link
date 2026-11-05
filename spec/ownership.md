# Ownership and threads

The host owns the JVM throughout its lifetime. A connection borrows VM access,
not a thread's JNI environment. Each operation receives an `Env` valid only for
its current thread and local frame, following the
[Java 21 JNI rules](https://docs.oracle.com/en/java/javase/21/docs/specs/jni/design.html).

## Resource ownership

| Resource | Owner | End of lifetime |
| --- | --- | --- |
| JVM and host module | Host | Host-controlled, after all borrowed access ends. |
| Connection state | `Connection` | Explicit shutdown followed by Rust destruction. |
| Pre-existing attachment | Host or another library | Never detached by jvm-link. |
| Attachment created by jvm-link | Current thread's state | Last owning scope exits; failed detach remains pending for that thread to retry. |
| Environment and local references | Thread-local scope/frame | Before scope exit, including errors and unwinding. |
| Exported global reference | Binding consumer | Binding-defined destruction, before host shutdown. |

The host-liveness obligation includes global references stored inside errors.
With `jvmti`, a session owns its environment and borrows its connection until
explicit close or Drop. Its reservation counts as active work, so connection
shutdown returns `Busy` while it exists. Session operations use the shared JNI
scope machinery for attachment and exception handling; no JNI environment is
stored. A session remains usable for cleanup when its connection is Closing.
Failed session Drop transfers ownership and the error to the VM's cleanup registry.
The same owner thread retries through connection close; environments and errors
survive connection Drop. Other threads can inspect ownership but cannot take the
queue. While a retry runs outside the lock its owner remains visible to shutdown.
Exported globals may outlive the connection but never the JVM. They keep Java
objects alive until released; consumers must control their lifetimes.

Use the binding's `Global<T>` directly: it owns one JNI global reference, which
its destructor deletes. It does not implement `Clone`; use `Arc<Global<T>>` if
Rust consumers need shared ownership, or `Env::new_global_ref` to create another
independently owned JNI global reference. Prefer dropping globals, including
throwables in errors, inside an attached scope. On a detached thread the binding
may temporarily attach to delete them, with its own cleanup behavior; these
binding-managed attachments are not tracked as jvm-link scope ownership. Local
references are released by the binding frame on success, errors and unwinding;
dropping a plain local wrapper alone does not delete the JNI reference.

## Scoped execution

1. Admit and count the scope under the same lock as the shutdown state. Release
   that lock before JNI execution or user callbacks.
2. Call `GetEnv`. Borrow an existing attachment without changing its ownership.
   Only on `JNI_EDETACHED`, call `AttachCurrentThreadAsDaemon`; stop on other errors.
3. Track ownership and nested scope depth in thread-local state. Open a fresh
   local frame and pass its environment to the closure.
4. Handle exceptions and release the frame before detachment. Only the last
   owning scope detaches; an inner scope never detaches an outer scope's thread.
5. Decrement the active scope count on success, error and unwinding.

Scopes nest in LIFO order. Do not use the outer environment while an inner scope
is active: the binding checks the current frame and may panic on incorrect order.
Never hold a connection-wide lock across Java calls or user callbacks, including
reentrant callbacks.

A scope cannot move between threads or survive an `await`. Local references
cannot escape it. A global reference may cross threads as allowed by its binding
type, but this does not make the referenced Java object safe for concurrent access.
Local frames bound temporary lifetimes, not allocations inside an unbounded loop;
use binding subframes and temporary-reference management when needed.

## Low-level attachment boundary

JL-3 implements this boundary privately in `src/scope.rs`. Its unsafe `with_env`
entry accepts a live VM and uses the binding's borrowed `AttachGuard` and fresh
local frame. Thread-local state is keyed by VM address, shared across nested
calls, and stores ownership/depth but never an environment. Failed detachments
remain owned at depth zero; `retry_cleanup` acts only on the current thread.
This also retains failures during unwinding. No TLS borrow spans JNI or a callback.

The `jni_version` field in the root `config.toml`, read and validated by
`src/config.rs`, selects JNI 1.8
for these baseline invocation calls; this
is an interface request, not Java runtime admission. The connection validates
Java 21 separately. The internal entry preserves an
exception pending on entry; JL-4 normalizes operation exceptions before frame
release and detachment. Public lifetime/thread checks now target `Connection`.

JL-5 adds `src/connection.rs`: an opaque public connection which discovers the
host VM and validates compatibility at its unsafe constructor. A short mutex-protected admission guard counts
scopes and releases its count after attachment cleanup, including unwinding.
No connection lock spans JNI or callbacks. A process-wide registry records
VM addresses, owning thread IDs and unreported cleanup errors, from successful
daemon attachment through successful detachment and error observation.
With JVMTI enabled it also retains failed session cleanup handles and errors.
It shares no JNI environment and tracks ownership across
connections to the same VM. Closing another connection cannot detach an active
outer scope; it reports `CleanupPending` until that owner finishes.

In `jni` 0.22.4, `AttachConfig` does not expose the required daemon mode.
`AttachGuard::drop` logs detach failures and explicit detachment consumes the
guard. Those helpers alone do not satisfy retryable cleanup.

Keep a narrow unsafe boundary for `GetEnv`, `AttachCurrentThreadAsDaemon`,
`DetachCurrentThread` and temporary conversion to a borrowed environment. Validate
return codes and pointers before conversion; reuse binding frames and reference
types afterward. jvm-link retains attachment ownership until successful detach
rather than losing it in a consumed guard. Test this boundary before relying on it.

References: [binding attachment source](https://docs.rs/jni/0.22.4/src/jni/vm/java_vm.rs.html),
[environment lifetimes](https://docs.rs/jni/0.22.4/jni/struct.Env.html), and
[Java 21 Invocation API](https://docs.oracle.com/en/java/javase/21/docs/specs/jni/invocation.html).
