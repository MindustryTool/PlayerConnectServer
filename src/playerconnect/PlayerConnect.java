package playerconnect;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

import arc.Events;
import arc.util.Log;
import arc.util.Log.LogLevel;

public class PlayerConnect {
    public static final NetworkRelay relay = new NetworkRelay();
    public static final ExecutorService executor = Executors.newSingleThreadExecutor();
    public static final HttpServer httpServer = new HttpServer();

    public static void main(String[] args) {
        try {
            Log.level = LogLevel.debug;
            int port = Integer.parseInt(System.getenv("PORT"));

            if (port < 0 || port > 0xffff)
                throw new RuntimeException("Invalid port range");
            // Init event loop

            relay.bind(port, port);
            Events.fire(new PlayerConnectEvents.ServerLoadedEvent());
            Log.info("Server loaded and hosted on port @. Type @ for help.", port, "'help'");

        } catch (Throwable t) {
            Log.err("Failed to load server", t);
            System.exit(1);
            return;
        }

        // Start the server
        try {
            relay.run();
        } catch (Throwable t) {
            Log.err(t);
        } finally {
            relay.close();
            Log.info("Server closed.");
        }
    }

}
