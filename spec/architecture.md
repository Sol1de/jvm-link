# Architecture

## JNI binding

Use [`jni` 0.22.4](https://docs.rs/crate/jni/0.22.4), with
`default-features = false` and no `invocation` feature. The prototype pins this
version and commits the resolved dependencies in `Cargo.lock`.

Reuse its environments, local frames, references and errors. `jni-sys` alone
would require unsafe wrappers for routine calls and reference handling; use
`jni::sys` only at the necessary low-level boundaries. Do not generate another
binding or build a replacement JNI SDK.

The future API reexports the binding as `jvm_link::jni`. Its types are therefore
part of the public contract: an incompatible binding upgrade requires a public
API compatibility review. Version 0.22 uses `Env` and `Global<T>`; do not copy
0.21 examples assuming the old `JNIEnv` API.

## Module boundaries

Only `src/lib.rs` exists in the prototype. The intended runtime layout is:

| Module | Responsibility |
| --- | --- |
| `lib.rs` | Public exports and API documentation. |
| `connection.rs` | Opaque borrowed connection, scope admission and shutdown state. |
| `vm_lookup.rs` | Resolve the lookup symbol in the loaded DLL and validate VM results. |
| `scope.rs` | Thread attachments, local frames and cleanup guards. |
| `compatibility.rs` | Java identification, JNI versions and checked capabilities. |
| `error.rs` | Structured public errors. |

Keep internals private. Replace the prototype trait with inherent methods on
`Connection` as the runtime is implemented, preserving its contract checks.

## JVM lookup and execution

JNI runs inside the JVM's process. The host must have loaded `jvm.dll` and started
its JVM. Resolve `JNI_GetCreatedJavaVMs` from that existing module; do not load a
new VM, invoke `JNI_CreateJavaVM` or call `DestroyJavaVM`.

Distinguish missing module, missing symbol, no VM, invalid pointer, multiple VMs
and JNI failure. Never choose an arbitrary VM from ambiguous results. Validate
codes and pointers before conversion, using `jni::sys` and the Windows system ABI.

Batch work inside a scope to avoid repeated attachment costs. Class names, method
signatures and class loaders remain JNI concerns: consumers supply the appropriate
loader rather than relying on `FindClass` from a native thread to find all classes.
Java calls may block, reenter native code or mutate the application; there is no
forced cancellation or isolation boundary.

## Java compatibility

Initially admit only Java 21. Future Java 8, 17 and other versions must share the
same lookup, connection, ownership and scope mechanisms. This means separate host
executions using different Java versions, not managing multiple JVMs in one process.

Centralize compatibility decisions internally. Retain each connection's detected
version and checked capabilities; do not assume Java 21 through global state or
duplicate the crate/backend per version. No plugin framework is needed.

The Rust binding version, JNI interface version and Java runtime version are
distinct. [`GetVersion`](https://docs.oracle.com/en/java/javase/21/docs/specs/jni/functions.html#getversion)
reports the JNI interface. Select the JNI version requested for `GetEnv` and
attachment centrally; check availability before using newer functions and keep
version-specific operations isolated from the common core.

During connection validation, use a technical scope to identify Java, including
[`java.specification.version`](https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/lang/System.html#getProperties())
(`1.8`, `17`, `21`, etc.). Host code can modify this property, so it never replaces
the JNI capability checks required for safe calls. Reject non-21 versions with
`UnsupportedJavaVersion`; report `JavaVersionUndetermined` if identification fails
rather than assuming 21. Preserve JNI causes and cleanup retry rules on failure.

Adding a supported version requires reviewing binding compatibility, actual JNI
calls and behavior differences, updating the centralized policy, and running
dedicated real-host tests. Connection API and ownership rules remain shared.

## Optional JVMTI

If implemented, put JVMTI behind a separate Cargo feature with an owned environment,
explicit capability requests and no callbacks by default. Define callback lifecycle
requirements if a consumer later needs them. Failed `DisposeEnvironment` must
retain the same environment for retry; the JNI core must work without this feature.
