# Public API

The authoritative signatures and error types are in [src/lib.rs](../src/lib.rs).
`ConnectionContract` is currently a compilation fixture without an implementation.
The runtime will expose inherent methods on an opaque `Connection`, not a public
backend trait or user-provided implementations.

## Connection operations

| Operation | Contract |
| --- | --- |
| `unsafe Connection::connect_existing()` | Discover and borrow the existing JVM, validate compatibility, and return a connection without creating a VM or retaining a permanent caller attachment. |
| `connection.with_env(operation)` | Run a synchronous closure in a fresh local frame, cleaned before returning its result. |
| `connection.close()` | Explicitly close owned resources without consuming the connection, allowing retries. |

The future `Connection` is `Send + Sync`, shareable by reference or `Arc`, without
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

The runtime will implement `Display` and `std::error::Error`; the prototype does
not define final diagnostic messages. Do not parse error text to decide retries.
See [lifecycle](lifecycle.md) for exception and cleanup policies.

## Contract validation

The doctests compile a valid consumer and reject four invalid cases: escaping an
environment, escaping a local reference, moving an environment to another thread,
and moving a local reference to another thread. Preserve these checks when moving
from the trait fixture to the real API. They do not exercise a JVM or prove the
runtime's ownership and cleanup behavior.
