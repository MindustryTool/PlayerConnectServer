1. Project & File Structure Rules

One responsibility per module

Each mod should have a single clear purpose.

Avoid “god modules”.

Predictable layout

src/ main.rs error.rs config.rs

Public API at the top

pub struct, pub enum, pub fn first

Helpers and private impls later

2. Naming Rules (Very Important for AI)

Use descriptive names, never single letters ❌ x, tmp, r, v ✅ request, buffer, user_id, packet_type

Avoid abbreviations ❌ cfg, mgr, svc ✅ config, manager, service

Types > comments

Prefer expressive type names over comments

Boolean names must answer yes/no

is_connected has_permission should_retry

3. Type System Rules

Prefer strong types over primitives

struct UserId(u64); struct Port(u16);

Never use String when an enum fits

enum Protocol { Tcp, Udp, }

Avoid Option<T> for required data

Use constructor validation instead

Avoid Vec<u8> without context

struct PacketBytes(Vec<u8>);

4. Error Handling Rules

Never use unwrap() or expect() in library code

OK only in tests or binaries

Use a single error enum

enum AppError { Io(std::io::Error), InvalidPacket, Timeout, }

Errors must be meaningful ❌ Err(AppError::Invalid) ✅ Err(AppError::InvalidPacketHeader)

Implement Display for errors

AI tools rely on readable messages

5. Function Design Rules

Functions should fit on one screen

~20–40 lines max

One logical action per function

Avoid hidden side effects

No silent global mutation

Explicit input > implicit state

fn encode(packet: &Packet, buffer: &mut BytesMut)

Prefer returning values over mutating inputs

6. Ownership & Borrowing Rules

Prefer borrowing over cloning

fn process(data: &Data)

Clone only at API boundaries

Avoid complex lifetime annotations

If lifetimes get hard, redesign

Use Arc<T> only for shared ownership

Never “just in case”

7. Async & Concurrency Rules

Never block in async code ❌ std::thread::sleep ✅ tokio::time::sleep

Name async functions clearly

async fn fetch_user()

One async runtime

Do not mix Tokio + async-std

Use channels instead of shared mutable state

8. Run cargo check before committing, and fix all errors
