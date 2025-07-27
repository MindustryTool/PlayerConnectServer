package playerconnect;

import java.io.IOException;

import arc.func.Prov;
import arc.net.DcReason;
import arc.struct.ArrayMap;
import arc.util.ArcRuntimeException;
import arc.util.io.ByteBufferInput;
import arc.util.io.ByteBufferOutput;

public class Packets {
    public static final byte id = -4;
    protected static final ArrayMap<Class<?>, Prov<? extends Packet>> packets = new ArrayMap<>();

    static {
        register(ConnectionPacketWrapPacket::new);
        register(ConnectionClosedPacket::new);
        register(ConnectionJoinPacket::new);
        register(ConnectionIdlingPacket::new);
        register(RoomCreationRequestPacket::new);
        register(RoomClosureRequestPacket::new);
        register(RoomClosedPacket::new);
        register(RoomLinkPacket::new);
        register(RoomJoinPacket::new);
        register(MessagePacket::new);
        register(Message2Packet::new);
        register(PopupPacket::new);
    }

    public static <T extends Packet> void register(Prov<T> cons) {
        packets.put(cons.get().getClass(), cons);
    }

    public static byte getId(Packet packet) {
        int id = packets.indexOfKey(packet.getClass());
        if (id == -1)
            throw new ArcRuntimeException("Unknown packet type: " + packet.getClass());
        return (byte) id;
    }

    @SuppressWarnings("unchecked")
    public static <T extends Packet> T newPacket(byte id) {
        if (id < 0 || id >= packets.size)
            throw new ArcRuntimeException("Unknown packet id: " + id);
        return ((Prov<T>) packets.getValueAt(id)).get();
    }

    /****************************/

    public static abstract class Packet {
        public void read(ByteBufferInput read) {
        };

        public void write(ByteBufferOutput write) {
        };
    }

    public static abstract class ConnectionWrapperPacket extends Packet {
        public int connectionId = -1;

        public void read(ByteBufferInput read) {
            connectionId = read.readInt();
            read0(read);
        }

        public void write(ByteBufferOutput write) {
            write.writeInt(connectionId);
            write0(write);
        }

        protected void read0(ByteBufferInput read) {
        };

        protected void write0(ByteBufferOutput write) {
        };
    }

    /** Special packet for connection packet wrapping. */
    public static class ConnectionPacketWrapPacket extends ConnectionWrapperPacket {
        /** serialization will be done by the proxy */
        public Object object;
        /** only for server usage */
        public java.nio.ByteBuffer buffer;

        public boolean isTCP;

        protected void read0(ByteBufferInput read) {
            isTCP = read.readBoolean();
        }

        protected void write0(ByteBufferOutput write) {
            write.writeBoolean(isTCP);
        }
    }

    public static class ConnectionClosedPacket extends ConnectionWrapperPacket {
        private static final DcReason[] reasons = DcReason.values();

        public DcReason reason;

        protected void read0(ByteBufferInput read) {
            reason = reasons[read.readByte()];
        }

        protected void write0(ByteBufferOutput write) {
            write.writeByte(reason.ordinal());
        }
    }

    public static class ConnectionJoinPacket extends ConnectionWrapperPacket {
        public String roomId = null;

        protected void read0(ByteBufferInput read) {
            try {
                roomId = read.readUTF();
            } catch (IOException e) {
                throw new RuntimeException(e);
            }
        }

        protected void write0(ByteBufferOutput write) {
            try {
                write.writeUTF(roomId);
            } catch (IOException e) {
                throw new RuntimeException(e);
            }
        }
    }

    public static class ConnectionIdlingPacket extends ConnectionWrapperPacket {
    }

    public static class RoomCreationRequestPacket extends Packet {
        public String version;

        public void read(ByteBufferInput read) {
            if (read.buffer.hasRemaining()) {
                try {
                    version = read.readUTF();
                } catch (Exception e) {
                    throw new RuntimeException(e);
                }
            }
        }

        public void write(ByteBufferOutput write) {
            try {
                write.writeUTF(version);
            } catch (Exception e) {
                throw new RuntimeException(e);
            }
        }
    }

    public static class RoomClosureRequestPacket extends Packet {
    }

    public static class RoomClosedPacket extends Packet {
        public static enum CloseReason {
            /** Closed without reason */
            closed,
            /** Incompatible client */
            obsoleteClient,
            /** Old version */
            outdatedVersion,
            /** Server is shutting down */
            serverClosed;

            public static final CloseReason[] all = values();
        }

        public CloseReason reason;

        public void read(ByteBufferInput read) {
            reason = CloseReason.all[read.readByte()];
        }

        public void write(ByteBufferOutput write) {
            write.writeByte(reason.ordinal());
        }
    }

    public static class RoomLinkPacket extends Packet {
        public String roomId = null;

        public void read(ByteBufferInput read) {
            try {
                roomId = read.readUTF();
            } catch (IOException e) {
                throw new RuntimeException(e);
            }
        }

        public void write(ByteBufferOutput write) {
            try {
                write.writeUTF(roomId);
            } catch (IOException e) {
                throw new RuntimeException(e);
            }
        }
    }

    public static class RoomJoinPacket extends RoomLinkPacket {
    }

    public static class MessagePacket extends Packet {
        public String message;

        public void read(ByteBufferInput read) {
            try {
                message = read.readUTF();
            } catch (Exception e) {
                throw new RuntimeException(e);
            }
        }

        public void write(ByteBufferOutput write) {
            try {
                write.writeUTF(message);
            } catch (Exception e) {
                throw new RuntimeException(e);
            }
        }
    }

    public static class Message2Packet extends Packet {
        public static enum MessageType {
            serverClosing,
            packetSpamming,
            alreadyHosting,
            roomClosureDenied,
            conClosureDenied;

            public static final MessageType[] all = values();
        }

        public MessageType message;

        public void read(ByteBufferInput read) {
            message = MessageType.all[read.readByte()];
        }

        public void write(ByteBufferOutput write) {
            write.writeByte(message.ordinal());
        }
    }

    public static class PopupPacket extends MessagePacket {
    }
}
