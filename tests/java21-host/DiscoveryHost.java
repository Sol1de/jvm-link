public final class DiscoveryHost {
    private static native boolean check();

    public static void fail() {
        throw new IllegalStateException("host operation failed");
    }

    public static void main(String[] args) {
        if (Runtime.version().feature() != 21) {
            throw new AssertionError("This technical host requires Java 21");
        }
        System.load(args[0]);
        if (!check()) {
            throw new AssertionError("Discovery did not return the host JVM");
        }
        // The Java thread and VM must still work after repeated discovery.
        System.out.println("Java 21 discovery and attachment scopes passed; host remains alive");
    }
}
