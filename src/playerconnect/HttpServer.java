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

import lombok.Data;

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
                Log.info("Client removed: <" + client + ">");
                statsConsumers.remove(client);
            });

            ArrayList<StatsLiveEvent> data = PlayerConnect.relay.rooms
                    .values()
                    .toSeq()
                    .map(room -> toLiveData(room, room.stats))
                    .list();

            Log.info("Client connected <" + client + "> sending " + data.size() + " rooms");

            client.sendEvent("update", data);

            statsConsumers.add(client);
        });

        app.start(Integer.parseInt(System.getenv("HTTP_PORT")));

        Events.on(PlayerConnectEvents.RoomCreatedEvent.class, event -> {
            try {
                sendUpdateEvent(toLiveData(event.room, event.room.stats));
            } catch (Exception error) {
                Log.err("Failed to send initial room stats", error);
            }
        });

        Events.on(Packets.StatsPacket.class, event -> {
            try {
                ServerRoom room = PlayerConnect.relay.rooms.get(event.roomId);

                if (room != null) {
                    sendUpdateEvent(toLiveData(room, event.data));
                } else {
                    Log.warn("Update stats for non-existent room");
                }
            } catch (Exception error) {
                Log.err("Failed to send room stats", error);
            }
        });

        Events.on(RoomClosedEvent.class, event -> {
            sendRemoveEvent(event.room.id);
        });

        scheduler.scheduleWithFixedDelay(() -> {
            statsConsumers.forEach(client -> client.sendComment("Kept alive"));
        }, 0, 1, TimeUnit.SECONDS);
    }

    private void sendUpdateEvent(StatsLiveEvent stat) {
        Log.info("Send update event: " + stat);

        ArrayList<StatsLiveEvent> response = new ArrayList<>();

        response.add(stat);

        for (SseClient client : statsConsumers) {
            client.sendEvent("update", response);
        }
    }

    private void sendRemoveEvent(String roomId) {
        Log.info("Sent remove event for " + roomId);

        HashMap<String, String> response = new HashMap<>();

        response.put("roomId", roomId);

        for (SseClient client : statsConsumers) {
            client.sendEvent("remove", response);
        }
    }

    private StatsLiveEvent toLiveData(ServerRoom room, RoomStats stats) {
        StatsLiveEvent response = new StatsLiveEvent();

        StatsLiveEventData data = new StatsLiveEventData();

        data.mapName = stats.mapName;
        data.name = stats.name;
        data.gamemode = stats.gamemode;
        data.mods = stats.mods.list();
        data.isSecured = room.password != null && !room.password.isEmpty();
        data.locale = stats.locale;
        data.version = stats.version;
        data.createdAt = room.createdAt;

        for (Packets.RoomPlayer playerData : stats.players) {
            StatsLiveEventPlayerData player = new StatsLiveEventPlayerData();
            player.name = playerData.name;
            player.locale = playerData.locale;
            data.players.add(player);
        }

        response.roomId = room.id;
        response.data = data;

        return response;
    }

    @Data
    private static class StatsLiveEvent {
        public String roomId = null;
        public StatsLiveEventData data;
    }

    @Data
    private static class StatsLiveEventData {
        public String name = "";
        public String status = "UP";
        public boolean isPrivate = false;
        public boolean isSecured = false;
        public ArrayList<StatsLiveEventPlayerData> players = new ArrayList<>();
        public String mapName = "unknown";
        public String gamemode = Gamemode.survival.name();
        public ArrayList<String> mods = new ArrayList<>();
        public String locale;
        public String version;
        public Long createdAt;
    }

    @Data
    private static class StatsLiveEventPlayerData {
        public String name = "";
        public String locale = "en";
    }
}
