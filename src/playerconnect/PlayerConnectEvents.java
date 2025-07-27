package playerconnect;

import arc.net.Connection;
import arc.net.DcReason;
import playerconnect.shared.Packets;

public class PlayerConnectEvents {
    public static class ServerLoadedEvent {
    }

    public static class ServerStoppingEvent {
    }

    public static class ClientConnectedEvent {
        public final Connection connection;

        public ClientConnectedEvent(Connection connection) {
            this.connection = connection;
        }
    }

    /**
     * @apiNote this event comes after {@link RoomClosedEvent} if the connection was
     *          the room host.
     */
    public static class ClientDisconnectedEvent {
        public final Connection connection;
        public final DcReason reason;
        /** not {@code null} if the client was in a room */
        public final ServerRoom room;

        public ClientDisconnectedEvent(Connection connection, DcReason reason, ServerRoom room) {
            this.connection = connection;
            this.reason = reason;
            this.room = room;
        }
    }

    /** Currently the only reason is for packet spam. */
    public static class ClientKickedEvent {
        public final Connection connection;

        public ClientKickedEvent(Connection connection) {
            this.connection = connection;
        }
    }

    /** When a connection join a room */
    public static class ConnectionJoinedEvent {
        public final Connection connection;
        public final ServerRoom room;

        public ConnectionJoinedEvent(Connection connection, ServerRoom room) {
            this.connection = connection;
            this.room = room;
        }
    }

    public static class RoomCreatedEvent {
        public final ServerRoom room;

        public RoomCreatedEvent(ServerRoom room) {
            this.room = room;
        }
    }

    public static class RoomClosedEvent {
        /** @apiNote the room is closed, so it cannot be used anymore. */
        public final ServerRoom room;

        public RoomClosedEvent(ServerRoom room) {
            this.room = room;
        }
    }

    public static class RoomCreationRejectedEvent {
        /** the connection that tried to create the room */
        public final Connection connection;
        public final Packets.RoomClosedPacket.CloseReason reason;

        public RoomCreationRejectedEvent(Connection connection, Packets.RoomClosedPacket.CloseReason reason) {
            this.connection = connection;
            this.reason = reason;
        }
    }

    /**
     * Defines an action tried by a connection but was not allowed to do it.
     * <p>
     * E.g. a client of the room tried to close it, or the host tried to join
     * another room while hosting one.
     */
    public static class ActionDeniedEvent {
        public final Connection connection;
        public final Packets.Message2Packet.MessageType reason;

        public ActionDeniedEvent(Connection connection, Packets.Message2Packet.MessageType reason) {
            this.connection = connection;
            this.reason = reason;
        }
    }
}
