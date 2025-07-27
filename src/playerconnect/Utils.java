package playerconnect;

import arc.net.Connection;

public class Utils {

    public static String getIP(Connection con) {
        java.net.InetSocketAddress a = con.getRemoteAddressTCP();
        return a == null ? null : a.getAddress().getHostAddress();
    }

    public static String toString(Connection con) {
        return "0x" + Integer.toHexString(con.getID());
    }
}
