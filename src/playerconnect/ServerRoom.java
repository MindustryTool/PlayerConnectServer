package playerconnect;

import java.nio.ByteBuffer;
import java.util.Date;
import java.util.UUID;

import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.IntMap;
import arc.util.Log;
import playerconnect.shared.Packets;

public class ServerRoom implements NetListener {

    public final String id;
    public final Connection host;
    public final String password;
    /** Using IntMap instead of Seq for faster search */
    public Packets.RoomStats stats;
    public Long ping = -1L;
    private boolean isClosed;
    public final IntMap<Connection> clients = new IntMap<>();
    public final Long createdAt = System.currentTimeMillis();

    public ServerRoom(Connection host, String password, Packets.RoomStats stats) {
        this.id = UUID.randomUUID().toString();
        this.host = host;
        this.stats = stats;
        this.password = password;
        this.ping = new Date().getTime() - stats.createdAt;
    }

    @Override
    public void connected(Connection connection) {
        if (isClosed)
            return;

        Packets.ConnectionJoinPacket p = new Packets.ConnectionJoinPacket();
        p.connectionId = connection.getID();
        p.roomId = id;
        host.sendTCP(p);

        clients.put(connection.getID(), connection);
    }

    /** Alert the host that a client disconnected */
    @Override
    public void disconnected(Connection connection, DcReason reason) {
        if (isClosed)
            return;

        if (connection == host) {
            close();
            return;

        } else if (host.isConnected()) {
            Packets.ConnectionClosedPacket p = new Packets.ConnectionClosedPacket();
            p.connectionId = connection.getID();
            p.reason = reason;
            host.sendTCP(p);
        }

        clients.remove(connection.getID());
    }

    /** Doesn't notify the room host about a disconnected client */
    public void disconnectedQuietly(Connection connection, DcReason reason) {
        if (isClosed)
            return;

        if (connection == host)
            close();
        else
            clients.remove(connection.getID());
    }

    /**
     * Wrap and re-send the packet to the host, if it come from a connection,
     * else un-wrap and re-send the packet to the specified connection. <br>
     * Only {@link Packets.ConnectionPacketWrapPacket} and {@link ByteBuffer}
     * are allowed.
     */
    @Override
    public void received(Connection connection, Object object) {
        if (isClosed)
            return;

        if (connection == host) {
            // Only claj packets are allowed in the host's connection
            // and can only be ConnectionPacketWrapPacket at this point.
            if (!(object instanceof Packets.ConnectionPacketWrapPacket))
                return;

            int connectionId = ((Packets.ConnectionPacketWrapPacket) object).connectionId;
            Connection con = clients.get(connectionId);

            if (con != null && con.isConnected()) {
                boolean tcp = ((Packets.ConnectionPacketWrapPacket) object).isTCP;
                Object o = ((Packets.ConnectionPacketWrapPacket) object).buffer;

                if (tcp)
                    con.sendTCP(o);
                else
                    con.sendUDP(o);

                // Notify that this connection doesn't exist, this case normally never happen
            } else if (host.isConnected()) {
                Packets.ConnectionClosedPacket p = new Packets.ConnectionClosedPacket();
                p.connectionId = connectionId;
                p.reason = DcReason.error;
                host.sendTCP(p);
                Log.info("Closed connection: " + con + " ip: " + Utils.getIp(con));
            }

        } else if (host.isConnected() && clients.containsKey(connection.getID())) {
            // Only raw buffers are allowed here.
            // We never send claj packets to anyone other than the room host, framework
            // packets are ignored
            // and mindustry packets are saved as raw buffer.
            if (!(object instanceof ByteBuffer))
                return;

            Packets.ConnectionPacketWrapPacket p = new Packets.ConnectionPacketWrapPacket();
            p.connectionId = connection.getID();
            p.buffer = (ByteBuffer) object;
            host.sendTCP(p);

        }
    }

    /** Notify the host of an idle connection. */
    @Override
    public void idle(Connection connection) {
        if (isClosed)
            return;

        if (connection == host) {
            // Ignore if this is the room host

        } else if (host.isConnected() && clients.containsKey(connection.getID())) {
            Packets.ConnectionIdlingPacket p = new Packets.ConnectionIdlingPacket();
            p.connectionId = connection.getID();
            host.sendTCP(p);
        }
    }

    /** Notify the room id to the host. Must be called once. */
    public void create() {
        if (isClosed)
            return;

        // Assume the host is still connected
        Packets.RoomLinkPacket p = new Packets.RoomLinkPacket();
        p.roomId = id;
        host.sendTCP(p);
    }

    /** @return whether the room is isClosed or not */
    public boolean isClosed() {
        return isClosed;
    }

    public void close() {
        close(Packets.RoomClosedPacket.CloseReason.closed);
    }

    /**
     * Closes the room and disconnects the host and all clients.
     * The room object cannot be used anymore after this.
     */
    public void close(Packets.RoomClosedPacket.CloseReason reason) {
        if (isClosed)
            return;
        isClosed = true; // close before kicking connections, to avoid receiving events

        // Alert the close reason to the host
        Packets.RoomClosedPacket p = new Packets.RoomClosedPacket();
        p.reason = reason;
        host.sendTCP(p);

        host.close(DcReason.closed);
        clients.values().forEach(c -> c.close(DcReason.closed));
        clients.clear();

        Log.info("Room @ closed, reason @", id, reason);
    }

    /** Checks if the connection is the room host or one of his client */
    public boolean contains(Connection con) {
        if (isClosed || con == null)
            return false;
        if (con == host)
            return true;
        return clients.containsKey(con.getID());
    }

    /** Send a message to the host and clients. */
    public void message(String text) {
        if (isClosed)
            return;

        // Just send to host, it will re-send it properly to all clients
        Packets.MessagePacket p = new Packets.MessagePacket();
        p.message = text;
        host.sendTCP(p);
    }

    /** Send a message the host and clients. Will be translated by the room host. */
    public void message(Packets.Message2Packet.MessageType message) {
        if (isClosed)
            return;

        Packets.Message2Packet p = new Packets.Message2Packet();
        p.message = message;
        host.sendTCP(p);
    }

    /** Send a popup to the room host. */
    public void popup(String text) {
        if (isClosed)
            return;

        Packets.PopupPacket p = new Packets.PopupPacket();
        p.message = text;
        host.sendTCP(p);
    }
}
