# This is a complete rewrite of player connect protocol  (a protocol base on claj) in Rust

## Purpose

- Improve the performance of the player connect server
- Make the player connect server more readable, extendable
- Easier to moderate

## Structure

### App 

Handle main loop and stuff

### Http Server

Handle all related http stuff

### Proxy Server

Handle proxy all game packet

### Event Bus 

Boardcast event between all components in system

## Data Model

Room {
    id
    host_connection
    password (optional)
    ping: number
    isClosed: boolean
    createdAt: number
    updatedAt: number
    clients: list of Connection
    stats: Stats
}

Stats {
    players: list of Player
    mapName: string
    name: string
    gamemode:string
    mods: string[]
    locale: string
    version: string
    createdAt: number
}

Player {
    name: string
    locale: string
}

ConnectionWrapperPacket {
  connectionId: int
}

ConnectionPacketWrapPacket (extends ConnectionWrapperPacket)
ConnectionPacketWrapPacket {
  object: Object            // non-primitive (generic object payload)
  buffer: ByteBuffer        // non-primitive (java.nio buffer)
  isTCP: boolean
}

ConnectionClosedPacket (extends ConnectionWrapperPacket)
ConnectionClosedPacket {
  reason: DcReason           // non-primitive (enum)
}

ConnectionJoinPacket (extends ConnectionWrapperPacket)
ConnectionJoinPacket {
  roomId: String             // non-primitive
}

ConnectionIdlingPacket (extends ConnectionWrapperPacket)
ConnectionIdlingPacket {
  // no additional fields
}

RoomLinkPacket (extends Packet)
RoomLinkPacket {
  roomId: String             // non-primitive
}

RoomJoinPacket (extends RoomLinkPacket)
RoomJoinPacket {
  password: String           // non-primitive
}

RoomClosureRequestPacket (extends Packet)
RoomClosureRequestPacket {
  // no fields
}

RoomClosedPacket (extends Packet)
RoomClosedPacket {
  reason: CloseReason        // non-primitive (enum)
}

RoomCreationRequestPacket (extends Packet)
RoomCreationRequestPacket {
  version: String            // non-primitive
  password: String           // non-primitive
  data: RoomStats            // non-primitive (object)
}

MessagePacket (extends Packet)
MessagePacket {
  message: String            // non-primitive
}

PopupPacket (extends MessagePacket)
PopupPacket {
  // inherits message
}

Message2Packet (extends Packet)
Message2Packet {
  message: MessageType       // non-primitive (enum)
}

StatsPacket (extends Packet)
StatsPacket {
  roomId: String             // non-primitive
  data: Stats            // non-primitive (object)
}

Enums
enum MessageType {
  serverClosing
  packetSpamming
  alreadyHosting
  roomClosureDenied
  conClosureDenied
}

enum CloseReason {
  closed
  obsoleteClient
  outdatedVersion
  serverClosed
}

## Flow

### App 

- Read env PLAYER_CONNECT_PORT, PLAYER_CONNECT_HTTP_PORT from env, validate
- Do initiation, logging, create HTTP server, bind to PLAYER_CONNECT_HTTP_PORT, create proxy server
- Start main loop

### Http Server

- Setup server
- Create routes:
+ GET /api/v1/rooms is a server sent event route that: 
    - On connected: send all rooms to client as and "update" server sent event
    - Listen to any room changes broadcast by proxy server and send as "update" server sent event
    - Listen to room deletion by proxy server and send an "remove" server sent event

+ GET /api/v1/ping: return 200 OK
+ GET /{roomId}:
    - Return a html page that show room info with metadata and opengraph
+ POST /{roomId}:
    - Return PLAYER_CONNECT_PORT

### Proxy Server
