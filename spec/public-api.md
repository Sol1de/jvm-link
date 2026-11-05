# Public API

The authoritative signatures and error types are in [src/lib.rs](../src/lib.rs).
`Connection` is an opaque public type with inherent methods. It replaces the
temporary `ConnectionContract` fixture; no public backend trait is exposed.

## Connection operations

| Operation | Contract |
| --- | --- |
| `unsafe Connection::connect_existing()` | Discover and borrow the existing JVM, validate compatibility, and return a connection without creating a VM or retaining a permanent caller attachment. |
| `connection.with_env(operation)` | Run a synchronous closure in a fresh local frame, cleaned before returning its result. |
| `connection.close()` | Explicitly close owned resources without consuming the connection, allowing retries. |

`Connection` is `Send + Sync`, shareable by reference or `Arc`, without
its own `Clone` implementation. The scoped operation accepts
`for<'local> FnOnce(&mut Env<'local>) -> Result<T>`. `T` is independent of the local
frame lifetime: owned Rust values and global references may escape; local
environments and references may not.

No asynchronous scope, stored environment, raw VM accessor, VM creation or VM
destruction method is exposed by the facade. The full binding remains available
inside the closure. Guarantees cover resources managed by jvm-link; permanent
attachments or other resources created directly by consumers remain their
responsibility.

## Why connection is unsafe

Discovering a valid VM pointer does not prove the host will keep it alive. The
caller must keep both JVM and `jvm.dll` available until all scopes, cleanup and
global references have ended, including globals held by errors. Successful
connection shutdown does not end this obligation while exported globals remain.

A fully safe constructor would require a host-provided lifetime protocol outside
this scope. Retaining the DLL alone does not prove the VM is still running.
Routine operations after connection should require no additional unsafe code
under this constructor precondition. See [ownership](ownership.md).

## Structured errors

With the optional `jvmti` feature, `connection.jvmti_session()` reserves a
current-thread `JvmtiSession` borrowing that connection. It performs no JVMTI
call until `session.open(&JvmtiCapabilities)`; pass `Default::default()` for no
capabilities. Repeated open calls add only explicitly requested capabilities to
the same environment. A failed open leaves the session available for cleanup,
including capability rejection or simultaneous attachment cleanup failure.
`session.close(&mut self)` is explicit, idempotent and retryable. Closing prohibits
further open calls. The session exposes neither callbacks nor a raw handle and
cannot be transferred or shared between threads. `JvmtiCapabilities` reuses the
binding bitfield type; unsupported requests are left to the host to reject.
Failed Drop cleanup remains owned in the VM cleanup registry. Retry connection
close on the session's thread, using the original or a later connection to that
VM. A remote thread receives `CleanupPending`. Successful disposal does not hide
an unreported Drop error: observe it, then close again to finish.

JVMTI function failures return `Jvmti { operation, code }`, retaining the numeric
JVMTI code (including unknown codes); malformed pointers or absent function slots
return `InvalidJvmtiEnvironment`. GetEnv failures retain their typed JNI cause
as `Operation`. These variants and exports exist only with the feature enabled.

Public error enums are non-exhaustive. They distinguish:

- JVM lookup failures: module/symbol absent, no VM, invalid VM pointer, multiple
  VMs, invalid returned counts (`InvalidVmCount { count }`) or a JNI error
  (`DiscoveryError`, produced by the internal `vm_lookup` module).
- Unsupported or undetermined Java versions.
- A closing/closed connection or still-active scopes.
- An exception already pending before an operation.
- Attachment, frame and operation errors retaining their typed JNI causes.
- Operation exceptions retaining a binding global throwable and any simultaneous
  operation error (`JavaException`); failed promotion retaining its typed JNI
  cause and operation error (`ExceptionCapture`).
- Cleanup failure with the owning thread and any simultaneous operation error.
- Pending cleanup owned by other threads.
- Constructor validation failure with outstanding cleanup (`ConnectionFailed`):
  the original structured error and a boxed closing connection are retained.
  Only `close` is permitted on this connection; retry on its owner thread.

Errors implement `Display` and `std::error::Error`, retaining typed causes.
Diagnostic text is not stable; do not parse it to decide retries.
See [lifecycle](lifecycle.md) for exception and cleanup policies.

## Contract validation

The doctests compile a valid consumer and reject four invalid cases: escaping an
environment, escaping a local reference, moving an environment to another thread,
and moving a local reference to another thread. These checks target the real
`Connection` API. They do not exercise a JVM or prove the
runtime's ownership and cleanup behavior.
