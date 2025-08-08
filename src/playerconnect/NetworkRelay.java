package playerconnect;

import java.nio.ByteBuffer;

import arc.Events;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.FrameworkMessage;
import arc.net.FrameworkMessage.*;
import arc.net.NetListener;
import arc.net.NetSerializer;
import arc.net.Server;
import arc.struct.IntMap;
import arc.struct.IntSet;
import arc.struct.ObjectMap;
import arc.util.Log;
import arc.util.Ratekeeper;
import arc.util.io.ByteBufferInput;
import arc.util.io.ByteBufferOutput;
import playerconnect.shared.Packets;

public class NetworkRelay extends Server implements NetListener {
    protected boolean isClosed;
    /**
     * Keeps a cache of packets received from connections that are not yet in a
     * room. (queue of 3 packets)<br>
     * Because sometimes {@link Packets.RoomJoinPacket} comes after
     * {@link Packets.ConnectPacket},
     * when the client connection is slow, so the server will ignore this essential
     * packet and the client
     * will waits until the timeout.
     */
    protected final IntMap<ByteBuffer[]> packetQueue = new IntMap<>();
    /** Size of the packet queue */
    protected final int packetQueueSize = 3;
    /**
     * Keeps a cache of already notified idling connection, to avoid packet
     * spamming.
     */
    protected final IntSet notifiedIdle = new IntSet();
    /** List of created rooms */
    public final ObjectMap<String, ServerRoom> rooms = new ObjectMap<>();

    public NetworkRelay() {
        super(32768, 16384, new Serializer());
        addListener(this);
    }

    @Override
    public void run() {
        isClosed = false;
        super.run();
    }

    @Override
    public void stop() {
        isClosed = true;
        Events.fire(new PlayerConnectEvents.ServerStoppingEvent());
        closeRooms();
        super.stop();
    }

    public boolean isClosed() {
        return isClosed;
    }

    public void closeRooms() {
        try {
            rooms.values().forEach(r -> {
                r.close(Packets.RoomClosedPacket.CloseReason.serverClosed);
                Events.fire(new PlayerConnectEvents.RoomClosedEvent(r));
            });
        } catch (Throwable ignored) {
        }
        rooms.clear();
    }

    @Override
    public void connected(Connection connection) {
        if (isClosed()) {
            Log.info("Connection @ denied, server closed.", Utils.toString(connection));
            connection.close(DcReason.closed);
            return;
        }

        if (Configs.IP_BLACK_LIST.contains(Utils.getIP(connection))) {
            Log.info("Connection @ denied, ip banned: @", Utils.toString(connection), Utils.getIP(connection));
            connection.close(DcReason.closed);
            return;
        }

        Log.info("Connection @ received.", Utils.toString(connection));
        connection.setArbitraryData(new Ratekeeper());
        connection.setName("Connection" + Utils.toString(connection)); // fix id format in stacktraces
        Events.fire(new PlayerConnectEvents.ClientConnectedEvent(connection));
    }

    @Override
    public void disconnected(Connection connection, DcReason reason) {
        Log.info("Connection @ lost: @.", Utils.toString(connection), reason);

        if (connection.getLastProtocolError() != null) {
            Log.err(connection.getLastProtocolError());
        }

        notifiedIdle.remove(connection.getID());
        packetQueue.remove(connection.getID());

        // Avoid searching for a room if it was an invalid connection or just a ping
        if (!(connection.getArbitraryData() instanceof Ratekeeper))
            return;

        ServerRoom room = find(connection);

        if (room != null) {
            room.disconnected(connection, reason);
            // Remove the room if it was the host

            if (connection == room.host) {
                rooms.remove(room.id);
                Log.info("Room @ closed because connection @ (the host) has disconnected.", room.id,
                        Utils.toString(connection));
                Events.fire(new PlayerConnectEvents.RoomClosedEvent(room));
            } else
                Log.info("Connection @ left the room @.", Utils.toString(connection), room.id);
        }

        Events.fire(new PlayerConnectEvents.ClientDisconnectedEvent(connection, reason, room));
    }

    @Override
    public void received(Connection connection, Object object) {
        try {

            if (!(connection.getArbitraryData() instanceof Ratekeeper) || (object instanceof FrameworkMessage))
                return;

            notifiedIdle.remove(connection.getID());

            Ratekeeper rate = (Ratekeeper) connection.getArbitraryData();
            ServerRoom room = find(connection);

            // Simple packet spam protection, ignored for room hosts
            if ((room == null || room.host != connection) && Configs.SPAM_LIMIT > 0
                    && !rate.allow(3000L, Configs.SPAM_LIMIT)) {

                if (room != null) {
                    room.message(Packets.Message2Packet.MessageType.packetSpamming);
                    room.disconnected(connection, DcReason.closed);
                }

                connection.close(DcReason.closed);
                Log.warn("Connection @ disconnected for packet spamming.", Utils.toString(connection));
                Events.fire(new PlayerConnectEvents.ClientKickedEvent(connection));

            } else if (object instanceof Packets.StatsPacket) {
                Packets.StatsPacket statsPacket = (Packets.StatsPacket) object;
                if (room != null) {
                    room.stats = statsPacket.data;
                    Events.fire(statsPacket.data);
                }
            } else if (object instanceof Packets.RoomJoinPacket) {
                Packets.RoomJoinPacket joinPacket = (Packets.RoomJoinPacket) object;
                // Disconnect from a potential another room.
                if (room != null) {
                    // Ignore if it's the host of another room
                    if (room.host == connection) {
                        room.message(Packets.Message2Packet.MessageType.alreadyHosting);
                        Log.warn("Connection @ tried to join the room @ but is already hosting the room @.",
                                Utils.toString(connection), joinPacket.roomId, room.id);
                        Events.fire(new PlayerConnectEvents.ActionDeniedEvent(connection,
                                Packets.Message2Packet.MessageType.alreadyHosting));
                        return;
                    }
                    // Disconnect from the room
                    Log.info("Connection @ left the room @ when trying to join", Utils.toString(connection), room.id);
                    room.disconnected(connection, DcReason.closed);
                }

                room = get(((Packets.RoomJoinPacket) object).roomId);
                if (room != null) {
                    if (!room.password.equals(joinPacket.password)) {
                        Packets.MessagePacket p = new Packets.MessagePacket();
                        p.message = "Wrong password";
                        connection.sendTCP(p);
                        connection.close(DcReason.error);
                        Log.info("Connection @ tried to join the room @ with wrong password.",
                                Utils.toString(connection), room.id);
                        return;
                    }

                    room.connected(connection);
                    Log.info("Connection @ joined the room @.", Utils.toString(connection), room.id);

                    // Send the queued packets of connections to room host
                    ByteBuffer[] queue = packetQueue.remove(connection.getID());
                    if (queue != null) {
                        Log.debug("Sending queued packets of connection @ to room host.",
                                Utils.toString(connection));
                        for (int i = 0; i < queue.length; i++) {
                            if (queue[i] != null)
                                room.received(connection, queue[i]);
                        }
                    }

                    Events.fire(new PlayerConnectEvents.ConnectionJoinedEvent(connection, room));
                } else {
                    connection.close(DcReason.error);
                    Log.info("Connection @ tried to join a non-existent room @.",
                            Utils.toString(connection), joinPacket.roomId);
                    Log.info("Room list: @", rooms.values().toSeq().map(r -> r.id).toString());
                }

            } else if (object instanceof Packets.RoomCreationRequestPacket) {
                // Ignore room creation requests when the server is closing
                Packets.RoomCreationRequestPacket packet = (Packets.RoomCreationRequestPacket) object;

                if (isClosed()) {
                    Packets.RoomClosedPacket p = new Packets.RoomClosedPacket();
                    p.reason = Packets.RoomClosedPacket.CloseReason.serverClosed;
                    connection.sendTCP(p);
                    connection.close(DcReason.error);
                    Events.fire(new PlayerConnectEvents.RoomCreationRejectedEvent(connection, p.reason));
                    Log.info("Ignore room creation, server is closing");
                    return;
                }

                // Ignore if the connection is already in a room or hold one
                if (room != null) {
                    room.message(Packets.Message2Packet.MessageType.alreadyHosting);
                    Log.warn("Connection @ tried to create a room but is already hosting the room @.",
                            Utils.toString(connection), room.id);
                    Events.fire(new PlayerConnectEvents.ActionDeniedEvent(connection,
                            Packets.Message2Packet.MessageType.alreadyHosting));
                    return;
                }

                room = new ServerRoom(connection, packet.password, packet.data);
                rooms.put(room.id, room);
                room.create();
                Log.info("Room @ created by connection @.", room.id, Utils.toString(connection));
                Events.fire(new PlayerConnectEvents.RoomCreatedEvent(room));

            } else if (object instanceof Packets.RoomClosureRequestPacket) {
                // Only room host can close the room
                if (room == null)
                    return;
                if (room.host != connection) {
                    room.message(Packets.Message2Packet.MessageType.roomClosureDenied);
                    Log.warn("Connection @ tried to close the room @ but is not the host.",
                            Utils.toString(connection),
                            room.id);
                    Events.fire(new PlayerConnectEvents.ActionDeniedEvent(connection,
                            Packets.Message2Packet.MessageType.roomClosureDenied));
                    return;
                }

                rooms.remove(room.id);
                room.close();
                Log.info("Room @ closed by connection @ (the host).", room.id, Utils.toString(connection));
                Events.fire(new PlayerConnectEvents.RoomClosedEvent(room));

            } else if (object instanceof Packets.ConnectionClosedPacket) {
                // Only room host can request a connection closing
                if (room == null)
                    return;

                Packets.ConnectionClosedPacket closePacket = (Packets.ConnectionClosedPacket) object;

                if (room.host != connection) {

                    room.message(Packets.Message2Packet.MessageType.conClosureDenied);
                    Log.warn("Connection @ tried to close the connection @ but is not the host of room @.",
                            Utils.toString(connection), closePacket.connectionId, room.id);
                    Events.fire(new PlayerConnectEvents.ActionDeniedEvent(connection,
                            Packets.Message2Packet.MessageType.conClosureDenied));
                    return;
                }

                int connectionId = closePacket.connectionId;
                Connection con = arc.util.Structs.find(getConnections(), c -> c.getID() == connectionId);
                DcReason reason = ((Packets.ConnectionClosedPacket) object).reason;

                // Ignore when trying to close itself or closing one that not in the same room
                if (con == connection || !room.contains(con)) {
                    Log.warn("Connection @ (room @) tried to close a connection from another room.",
                            Utils.toString(connection), room.id);
                    return;
                }

                if (con != null) {
                    Log.info("Connection @ (room @) closed the connection @.", Utils.toString(connection), room.id,
                            Utils.toString(con));
                    room.disconnectedQuietly(con, reason);
                    con.close(reason);
                    // An event for this is useless, #disconnected() will trigger it
                }

                // Ignore if the connection is not in a room
            } else if (room != null) {
                if (room.host == connection && (object instanceof Packets.ConnectionWrapperPacket))
                    notifiedIdle.remove(((Packets.ConnectionWrapperPacket) object).connectionId);

                room.received(connection, object);

                // Puts in queue; if full, future packets will be ignored.
            } else if (object instanceof ByteBuffer) {
                ByteBuffer[] queue = packetQueue.get(connection.getID(), () -> new ByteBuffer[packetQueueSize]);
                ByteBuffer buffer = (ByteBuffer) object;

                for (int i = 0; i < queue.length; i++) {
                    if (queue[i] == null) {
                        queue[i] = (ByteBuffer) ByteBuffer.allocate(buffer.remaining()).put(buffer).rewind();
                        break;
                    }
                }
            } else {
                Log.warn("Unhandled packet: @", object);
            }
        } catch (Exception error) {
            Log.err("Failed to handle: " + object, error);
        }
    }

    /**
     * Does nothing if the connection idle state was already notified to the room
     * host.
     */
    @Override
    public void idle(Connection connection) {
        if (!(connection.getArbitraryData() instanceof Ratekeeper))
            return;
        if (!notifiedIdle.add(connection.getID()))
            return;

        ServerRoom room = find(connection);
        if (room != null)
            room.idle(connection);
    }

    public ServerRoom get(String roomId) {
        return rooms.get(roomId);
    }

    public ServerRoom find(Connection con) {
        for (ServerRoom r : rooms.values()) {
            if (r.contains(con))
                return r;
        }
        return null;
    }

    public static class Serializer implements NetSerializer {
        private final ThreadLocal<ByteBuffer> last = arc.util.Threads.local(() -> ByteBuffer.allocate(16384));

        @Override
        public Object read(ByteBuffer buffer) {
            byte id = buffer.get();

            if (id == -2/* framework id */)
                return readFramework(buffer);

            if (id == Packets.id) {
                Packets.Packet packet = Packets.newPacket(buffer.get());
                packet.read(new ByteBufferInput(buffer));
                if (packet instanceof Packets.ConnectionPacketWrapPacket) // This one is special
                    ((Packets.ConnectionPacketWrapPacket) packet).buffer = (ByteBuffer) ((ByteBuffer) last.get()
                            .clear()).put(buffer).flip();
                return packet;
            }

            // Non-claj packets are saved as raw buffer, to avoid re-serialization
            return ((ByteBuffer) last.get().clear()).put((ByteBuffer) buffer.position(buffer.position() - 1)).flip();
        }

        @Override
        public void write(ByteBuffer buffer, Object object) {
            if (object instanceof ByteBuffer) {
                buffer.put((ByteBuffer) object);

            } else if (object instanceof FrameworkMessage) {
                buffer.put((byte) -2); // framework id
                writeFramework(buffer, (FrameworkMessage) object);

            } else if (object instanceof Packets.Packet) {
                Packets.Packet packet = (Packets.Packet) object;
                buffer.put(Packets.id).put(Packets.getId(packet));
                packet.write(new ByteBufferOutput(buffer));
                if (packet instanceof Packets.ConnectionPacketWrapPacket) // This one is special
                    buffer.put(((Packets.ConnectionPacketWrapPacket) packet).buffer);
            }
        }

        public void writeFramework(ByteBuffer buffer, FrameworkMessage message) {
            if (message instanceof Ping) {
                Ping ping = (Ping) message;
                buffer.put((byte) 0).putInt(ping.id).put(ping.isReply ? (byte) 1 : 0);
            } else if (message instanceof DiscoverHost)
                buffer.put((byte) 1);
            else if (message instanceof KeepAlive)
                buffer.put((byte) 2);
            else if (message instanceof RegisterUDP)
                buffer.put((byte) 3).putInt(((RegisterUDP) message).connectionID);
            else if (message instanceof RegisterTCP)
                buffer.put((byte) 4).putInt(((RegisterTCP) message).connectionID);
        }

        public FrameworkMessage readFramework(ByteBuffer buffer) {
            byte id = buffer.get();

            if (id == 0) {
                Ping p = new Ping();
                p.id = buffer.getInt();
                p.isReply = buffer.get() == 1;
                return p;
            } else if (id == 1) {
                return FrameworkMessage.discoverHost;
            } else if (id == 2) {
                return FrameworkMessage.keepAlive;
            } else if (id == 3) {
                RegisterUDP p = new RegisterUDP();
                p.connectionID = buffer.getInt();
                return p;
            } else if (id == 4) {
                RegisterTCP p = new RegisterTCP();
                p.connectionID = buffer.getInt();
                return p;
            } else {
                throw new RuntimeException("Unknown framework message!");
            }
        }
    }
}
