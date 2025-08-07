package playerconnect;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.Queue;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.SerializationFeature;

import arc.Events;
import arc.util.Log;
import io.javalin.Javalin;
import io.javalin.json.JavalinJackson;
import io.javalin.plugin.bundled.RouteOverviewPlugin;
import mindustry.game.Gamemode;
import playerconnect.PlayerConnectEvents.RoomClosedEvent;
import playerconnect.shared.Packets;
import playerconnect.shared.Packets.RoomStats;
import io.javalin.http.sse.SseClient;

public class HttpServer {

    private Javalin app;

    private final Queue<SseClient> statsConsumers = new ConcurrentLinkedQueue<>();
    private final ScheduledExecutorService scheduler = Executors.newSingleThreadScheduledExecutor();

    public HttpServer() {
        app = Javalin.create(config -> {
            config.showJavalinBanner = false;
            config.jsonMapper(new JavalinJackson().updateMapper(mapper -> {
                mapper//

                        .configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false)//
                        .configure(DeserializationFeature.ACCEPT_SINGLE_VALUE_AS_ARRAY, true)//
                        .configure(SerializationFeature.FAIL_ON_EMPTY_BEANS, false)
                        .configure(SerializationFeature.WRITE_DATES_AS_TIMESTAMPS, false);

            }));

            config.http.asyncTimeout = 5_000;
            config.useVirtualThreads = true;

            config.registerPlugin(new RouteOverviewPlugin());

            config.requestLogger.http((ctx, ms) -> {
                if (!ctx.fullUrl().contains("stats")) {
                    Log.info("[" + ctx.method().name() + "] " + Math.round(ms) + "ms " + ctx.fullUrl());
                }
            });
        });

        app.sse("rooms", client -> {
            client.keepAlive();

            client.onClose(() -> {
                statsConsumers.remove(client);
            });

            ArrayList<StatsLiveEventData> data = PlayerConnect.relay.rooms
                    .values()
                    .toSeq()
                    .map(room -> toLiveData(room.stats))
                    .list();

            client.sendEvent(data);

            statsConsumers.add(client);
        });

        app.start(Integer.parseInt(System.getenv("HTTP_PORT")));

        Events.on(PlayerConnectEvents.RoomCreatedEvent.class, event -> {
            StatsLiveEventData stat = toLiveData(event.room.stats);
            sendUpdateEvent(stat);
        });

        Events.on(Packets.StatsPacket.class, event -> {
            StatsLiveEventData stat = toLiveData(event.data);
            sendUpdateEvent(stat);
        });

        Events.on(RoomClosedEvent.class, event -> {
            sendRemoveEvent(event.room.id);
        });

        scheduler.scheduleWithFixedDelay(() -> {
            statsConsumers.forEach(client -> client.sendComment("Kept alive"));
        }, 0, 10, TimeUnit.SECONDS);
    }

    private void sendUpdateEvent(StatsLiveEventData stat) {
        ArrayList<StatsLiveEventData> response = new ArrayList<>();
        response.add(stat);

        for (SseClient client : statsConsumers) {
            client.sendEvent("update", response);
        }

        Log.info("Send update event: " + stat);
    }

    private void sendRemoveEvent(String roomId) {
        HashMap<String, String> response = new HashMap<>();

        response.put("roomId", roomId);

        for (SseClient client : statsConsumers) {
            client.sendEvent("remove", response);
        }

        Log.info("Sent remove event for " + roomId);
    }

    private StatsLiveEventData toLiveData(RoomStats data) {
        StatsLiveEventData stat = new StatsLiveEventData();
        stat.mapName = data.mapName;
        stat.name = data.name;
        stat.gamemode = data.gamemode;
        stat.mods = data.mods.list();
        stat.roomId = data.roomId;

        for (Packets.RoomPlayer playerData : data.players) {
            StatsLiveEventPlayerData player = new StatsLiveEventPlayerData();
            player.name = playerData.name;
            player.locale = playerData.locale;
            stat.players.add(player);
        }

        return stat;
    }

    public static class StatsLiveEventData {
        public ArrayList<StatsLiveEventPlayerData> players = new ArrayList<>();
        public String mapName = "unknown";
        public String name = "";
        public String gamemode = Gamemode.survival.name();
        public ArrayList<String> mods = new ArrayList<>();
        public String roomId = null;
    }

    public static class StatsLiveEventPlayerData {
        public String name = "";
        public String locale = "en";
    }
}
