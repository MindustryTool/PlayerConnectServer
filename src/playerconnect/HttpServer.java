package playerconnect;

import java.util.ArrayList;
import java.util.Queue;
import java.util.concurrent.ConcurrentLinkedQueue;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.SerializationFeature;

import arc.Events;
import arc.util.Log;
import io.javalin.Javalin;
import io.javalin.json.JavalinJackson;
import io.javalin.plugin.bundled.RouteOverviewPlugin;
import mindustry.game.Gamemode;
import playerconnect.shared.Packets;
import io.javalin.http.sse.SseClient;

public class HttpServer {

    private Javalin app;

    private final Queue<SseClient> statsConsumers = new ConcurrentLinkedQueue<>();

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
                    .map(room -> {
                        StatsLiveEventData response = new StatsLiveEventData();
                        response.roomId = room.id;
                        response.name = room.id;

                        return response;
                    }).list();

            client.sendEvent(data);

            statsConsumers.add(client);
        });

        app.start(Integer.parseInt(System.getenv("HTTP_PORT")));

        Events.on(PlayerConnectEvents.RoomCreatedEvent.class, event -> {
            StatsLiveEventData stat = new StatsLiveEventData();
            stat.roomId = event.room.id;
            sendStat(stat);
        });

        Events.on(Packets.StatsPacket.class, event -> {
            StatsLiveEventData stat = new StatsLiveEventData();
            stat.mapName = event.data.mapName;
            stat.name = event.data.name;
            stat.gamemode = event.data.gamemode;
            stat.mods = event.data.mods.list();
            stat.roomId = event.data.roomId;

            for (Packets.StatsPacketPlayerData playerData : event.data.players) {
                StatsLiveEventPlayerData player = new StatsLiveEventPlayerData();
                player.name = playerData.name;
                player.locale = playerData.locale;
                stat.players.add(player);
            }
            sendStat(stat);
        });
    }

    private void sendStat(StatsLiveEventData stat) {
        ArrayList<StatsLiveEventData> response = new ArrayList<>();
        response.add(stat);

        for (SseClient client : statsConsumers) {
            client.sendEvent(response);
        }
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
