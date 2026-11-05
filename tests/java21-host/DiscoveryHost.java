public final class DiscoveryHost {
    private static native boolean check();
    private static native int admission();

    public static void fail() {
        throw new IllegalStateException("host operation failed");
    }

    public static void main(String[] args) {
        if (Runtime.version().feature() != 21) {
            throw new AssertionError("This technical host requires Java 21");
        }
        System.load(args[0]);
        String original = System.getProperty("java.specification.version");
        try {
            for (String value : new String[] {"17", "1.8", "", "unknown", "21"}) {
                System.setProperty("java.specification.version", value);
                int expected = switch (value) {
                    case "17" -> -17;
                    case "1.8" -> -8;
                    case "21" -> 21;
                    default -> 0;
                };
                if (admission() != expected) {
                    throw new AssertionError("Wrong admission for " + value);
                }
            }
            System.clearProperty("java.specification.version");
            if (admission() != 0) {
                throw new AssertionError("Missing Java version was admitted");
            }
        } finally {
            System.setProperty("java.specification.version", original);
        }
        if (!check()) {
            throw new AssertionError("Discovery did not return the host JVM");
        }
        // The Java thread and VM must still work after repeated discovery.
        System.out.println("Java 21 discovery and attachment scopes passed; host remains alive");
    }
}
