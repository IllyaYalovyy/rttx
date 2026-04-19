# RFC-014: Cloud Relay Service

| Field         | Value                                    |
|---------------|------------------------------------------|
| Status        | Deferred                                 |
| Author(s)     | Illya Yalovyy                            |
| Supersedes    | —                                        |
| Superseded by | —                                        |

## Summary

A cloud relay service that brokers connections between rttx clients and rttx-server daemons over the internet, eliminating the need for SSH, open inbound ports, or VPN infrastructure. Daemons establish outbound-only WebSocket connections to the relay; clients connect to the same relay and are routed to their registered daemons. End-to-end encryption ensures the relay cannot read terminal content, while OAuth2-based user authentication and device registration provide access control. The relay is a dumb pipe by design - it forwards opaque frames without understanding the rttx protocol.

## Current implementation baseline (2026-04)

- The relay itself is unimplemented; this RFC remains a forward-looking design document.
- Implemented endpoint types today are local Unix-socket and remote SSH stdio.
- The monorepo consolidation (RFC-010) is complete: client, daemon, and protocol live in one
  workspace under `clients/rttx/`, `services/rttx-server/`, and `protocols/rttx-proto/`.
- The `DaemonConnection::into_split()` refactor (#136) is implemented: `DaemonConnection` accepts
  any `AsyncRead + AsyncWrite` stream and splits into `DaemonReader`/`DaemonWriter` for concurrent
  use by the endpoint actor.
- `EndpointConnectionManager` in `clients/rttx/src/daemon_bridge.rs` runs one async
  `EndpointActor` per endpoint, multiplexing workspaces and runtimes on a shared transport.
- The connection state machine (RFC-018) is implemented as `advance_connection_status` in
  `clients/rttx/src/runtime.rs` with durable states: `Starting`, `Connecting`, `Connected`,
  `Reconnecting`, `Blocked(ConnectionProblem)`, `Disconnected`, `Recovered`, `SessionMissing`.
- `ConnectionProblem` classifies daemon/protocol errors into 7 variants; only
  `DaemonUnavailable` is transient (auto-retryable).
- The wire protocol is v2 (protobuf over 4-byte LE length-prefixed frames). Protocol v3 direction
  is defined in RFC-021 with capability negotiation, structured command/event envelopes, and
  explicit ownership/discovery semantics.
- Workspace management v2 (RFC-016) provides the action-oriented creation model: `New`,
  `Connect to Existing`, `New Direct`.
- RFC-019 added `SessionMissing` state for workspaces whose daemon-side runtime has disappeared.
- RFC-020 ensures the daemon answers terminal queries independently of client attachment.
- RFC-022 proposes a v2 daemon state layout with per-session directories and schema versioning.
- RFC-023 proposes a redesigned client-side configuration and state store.
- No relay-related code exists in the codebase yet. Adding a relay endpoint type is a natural
  extension of the existing `EndpointActor` architecture.

## Terminology

| Term              | Definition |
|-------------------|------------|
| **Relay**         | The cloud service that routes frames between clients and daemons. Comprises a control plane and a data plane. |
| **Control plane** | The stateless API surface for authentication, device registration, and endpoint discovery. |
| **Data plane**    | The WebSocket relay process that forwards encrypted frames between matched client-daemon pairs. |
| **Device**        | A host running rttx-server that has been registered with the relay. Each device has a unique identity and keypair. |
| **Device token**  | A long-lived credential issued during device registration, used by the daemon to authenticate to the relay. |
| **Tunnel**        | A WebSocket connection between a component (client or daemon) and the relay data plane. |
| **Channel**       | The logical end-to-end encrypted connection between a client and a daemon, carried over two tunnels through the relay. |
| **Envelope**      | The thin routing header the relay uses to forward frames. Contains source/destination identifiers and opaque payload. |
| **TOFU**          | Trust On First Use. The client accepts the daemon's public key on first connection and pins it for subsequent connections. |

## Goals

- G1: Enable rttx clients to connect to daemons on any internet-accessible host without SSH, open inbound ports, or VPN
- G2: End-to-end encryption between client and daemon; the relay operator cannot read terminal content
- G3: Secure authentication and authorization: OAuth2/OIDC for users, device tokens for daemons, explicit device-to-user binding
- G4: Hybrid connectivity: a single rttx instance can simultaneously use local (Unix socket), SSH, and relay endpoints
- G5: Self-hostable: the relay runs as a single binary for users who want to operate their own infrastructure
- G6: Latency overhead under 50ms above baseline for the relay hop
- G7: File transfer between client and daemon without streaming through the relay
- G8: The existing rttx protocol (protobuf over length-prefixed frames) remains unchanged; the relay wraps it

## Non-Goals

- NG1: **Smart broker capabilities** - no store-and-forward, no server-side session listing, no protocol-aware message inspection. The relay does not understand rttx protocol messages. If these capabilities are needed later, they can be selectively promoted from the daemon to the relay.
- NG2: **Multi-user session sharing** - the relay enforces device ownership, but collaborative features (shared sessions, multi-cursor) are out of scope. The daemon's single-writer model is unchanged.
- NG3: **Built-in file storage** - the relay brokers metadata for file transfer; actual bytes flow through object storage (S3-compatible) or direct client-daemon channels.
- NG4: **Web-based terminal client** - the relay serves rttx (native GTK client) only. A web client would require the relay to terminate the E2E encryption, violating G2.
- NG5: **Replacing SSH** - SSH remains a first-class endpoint type. The relay is an additional connectivity option for environments where SSH is impractical.
- NG6: **Peer-to-peer connectivity** - WebRTC data channels or hole-punching for direct client-daemon connections. The relay always mediates. P2P can be explored as a future optimization.

## Background & Motivation

rttx currently supports two endpoint types: local (Unix socket to a co-located daemon) and remote (SSH to a daemon via `rttx-server attach-stdio`). The SSH model works well for hosts that the user can reach directly, but has significant limitations:

- **Inbound port requirements**: the remote host must accept SSH connections, which means open ports, firewall rules, and SSH hardening
- **Network topology constraints**: hosts behind NAT, corporate firewalls, or cloud VPCs require VPN, bastion hosts, or tunneling infrastructure
- **Key management burden**: SSH keys must be provisioned, rotated, and distributed across all client machines
- **No centralized access control**: authorization is managed per-host via `authorized_keys` or PAM, with no unified view of who can access what

A relay service inverts the connectivity model: daemons connect outbound to the relay (through any firewall that allows HTTPS), and clients connect to the same relay. The relay matches them based on authenticated identity. This eliminates all four limitations above while preserving the security properties that matter - the terminal content is end-to-end encrypted and the daemon's session ownership model is unchanged.

**Why now**: the monorepo consolidation (RFC-010) places the client, server, and protocol in a single workspace, making coordinated transport changes practical. The `DaemonConnection::into_split()` refactor (#136) already abstracts the transport boundary — `DaemonConnection` accepts any `AsyncRead + AsyncWrite` stream, so adding a WebSocket-backed stream is a drop-in change. The `EndpointConnectionManager` runs one async actor per endpoint with independent connection lifecycle, and `RuntimeEndpoint` already distinguishes `Local` and `Remote { host }` — adding a `Relay { device_id }` variant is a natural extension.

## User Impact

| User Segment | Impact |
|---|---|
| Developers accessing cloud VMs | Direct terminal access without SSH setup, VPN, or bastion hosts |
| Teams with shared infrastructure | Centralized device registry with OAuth2 authentication; no per-host key distribution |
| Security-conscious users | E2E encryption means trusting the relay operator is not required for confidentiality |
| Self-hosters | Single-binary relay for private infrastructure; no vendor lock-in |
| Existing SSH users | No impact; SSH endpoints continue to work unchanged alongside relay endpoints |

## Considered Options

### Option A: Dumb relay with E2E encryption (chosen)

The relay forwards opaque encrypted frames between authenticated WebSocket connections. It knows routing (which client talks to which daemon) but cannot read payload content.

**Pros:**
- Minimal attack surface: the relay has no access to terminal content
- Simple implementation: the data plane is a WebSocket frame router
- Trust model is clear: users only need to trust the relay for availability, not confidentiality
- Self-hosting is straightforward: small, stateless binary

**Cons:**
- Cannot provide offline capabilities (session listing when daemon is unreachable)
- File transfer requires a side channel (object storage)
- Future features like server-side search or notifications would require protocol awareness

### Option B: Smart broker that understands the rttx protocol

The relay decrypts, inspects, and can act on rttx protocol messages.

**Pros:**
- Can cache session lists, provide offline status, serve notifications
- File transfer can be handled directly
- Enables server-side features without daemon changes

**Cons:**
- Relay becomes a high-value target: it sees all terminal content
- Much larger implementation and operational surface
- Self-hosting is harder: more state, more failure modes
- Violates the principle of least privilege

### Option C: VPN-based approach (WireGuard/Tailscale integration)

Leverage existing VPN infrastructure to create direct network paths, then use standard Unix socket or SSH connections.

**Pros:**
- Reuses proven technology
- No custom relay infrastructure
- Full network connectivity (not just terminal)

**Cons:**
- External dependency (Tailscale account or WireGuard infrastructure)
- Complex setup: every host needs VPN client configuration
- rttx becomes dependent on third-party availability and pricing
- Does not solve the problem for users who want a self-contained solution

## Decision

**Option A: Dumb relay with E2E encryption.**

The relay's sole job is authenticated frame routing. Terminal content is end-to-end encrypted using the Noise protocol between client and daemon; the relay sees only ciphertext. This matches rttx's principle of composable building blocks (RFC-001 P4): the relay is a transport primitive, not an application layer.

The architecture explicitly supports promoting specific capabilities to the relay later (e.g., device presence, session count metadata) without redesigning the core. But the default is opacity.

## Requirements

### Connectivity

- R1: Daemons connect to the relay using outbound-only WebSocket (wss://) connections. No inbound ports are required on the daemon host.
- R2: Clients connect to the relay using the same WebSocket protocol. A single rttx instance can maintain simultaneous connections to local, SSH, and relay endpoints.
- R3: The relay forwards frames between a matched client-daemon pair without inspecting or modifying the payload.
- R4: If the relay process restarts, both clients and daemons reconnect automatically. In-flight channel state is lost (the Noise session must be re-established), but the daemon's rttx-server state is unaffected.

### Security

- R5: Terminal content is end-to-end encrypted using the Noise protocol (IK handshake pattern). The relay cannot decrypt payload content.
- R6: Users authenticate to the relay control plane via OAuth2/OIDC (GitHub, Google, or self-hosted provider). The control plane issues short-lived JWTs for relay API access.
- R7: Daemons authenticate to the relay using device tokens issued during a user-initiated registration flow. Device tokens are long-lived but revocable.
- R8: The relay enforces authorization: a client can only be routed to devices registered to the authenticated user (or explicitly shared with them).
- R9: Client-daemon identity verification uses TOFU with key pinning. On first connection, the client prompts the user to accept the daemon's public key. Subsequent connections verify against the pinned key.

### Performance

- R10: The relay hop adds no more than 50ms of latency above the baseline network RTT between client and relay, and relay and daemon.
- R11: The data plane relay process is stateless per-connection: no disk I/O, no database queries on the forwarding path. Connection metadata is held in memory only.

### File Transfer

- R12: File transfer uses pre-signed URLs with S3-compatible object storage. The daemon uploads to storage, the relay control plane brokers the metadata (pre-signed URL, filename, size), and the client downloads directly. File bytes never transit the relay data plane.
- R13: Transfer metadata messages are part of the E2E encrypted channel. The relay cannot observe filenames, sizes, or storage URLs.

### Operations

- R14: The relay is deployable as a single binary for self-hosted environments. Configuration is via environment variables or a single config file.
- R15: The managed (cloud-hosted) relay and self-hosted relay use the same binary and protocol. The only difference is the OAuth2/OIDC provider configuration and object storage backend.

## Design

### Architecture Overview

```
                         ┌──────────────────────────────────┐
                         │          Relay Service            │
                         │                                   │
  ┌─────────┐   wss://   │  ┌─────────────┐  ┌───────────┐  │   wss://    ┌─────────────┐
  │  rttx   │◄──────────►│  │  Data Plane  │  │  Control  │  │◄──────────► │ rttx-server │
  │ (GUI)   │            │  │  (WebSocket  │  │   Plane   │  │            │  (daemon)   │
  │         │   https://  │  │   relay)     │  │  (REST)   │  │   https://  │             │
  │         │◄──────────►│  │              │  │           │  │◄──────────► │             │
  └─────────┘            │  └─────────────┘  └───────────┘  │            └─────────────┘
                         └──────────────────────────────────┘
```

The service has two distinct planes:

**Control plane** (REST over HTTPS): handles authentication, device registration, device listing, and file transfer metadata. Stateless - suitable for serverless deployment (API Gateway + Lambda or equivalent).

**Data plane** (WebSocket): maintains persistent connections from clients and daemons, forwards encrypted frames between matched pairs. Long-lived process with in-memory connection state. Stateless on disk - if it restarts, connections simply reconnect.

### Control Plane API

#### Authentication

```
POST /auth/token
  Request:  OAuth2 authorization code exchange
  Response: { access_token: JWT, refresh_token, expires_in }

POST /auth/refresh
  Request:  { refresh_token }
  Response: { access_token: JWT, expires_in }
```

The JWT contains the user ID, email, and granted scopes. It is used for all subsequent control plane requests and for initial data plane authentication.

#### Device Registration

```
POST /devices/register
  Auth:     Bearer <JWT>
  Request:  { name, public_key, platform, hostname }
  Response: { device_id, device_token }

GET /devices
  Auth:     Bearer <JWT>
  Response: { devices: [{ device_id, name, platform, hostname, status, last_seen }] }

DELETE /devices/{device_id}
  Auth:     Bearer <JWT>
  Response: 204

POST /devices/{device_id}/regenerate-token
  Auth:     Bearer <JWT>
  Response: { device_token }
```

Registration flow:
1. User runs `rttx-server register --relay <url>` on the remote host
2. The command generates a Noise static keypair, stores the private key locally
3. The command opens a browser-based OAuth2 flow (or displays a device code for headless hosts)
4. After authentication, the control plane stores the device's public key and returns a device token
5. The daemon stores the device token and connects to the data plane on startup

#### Device Discovery

```
GET /devices/{device_id}/connect
  Auth:     Bearer <JWT>
  Response: { data_plane_url, connection_token, device_public_key }
```

The client calls this before initiating a data plane connection. The `connection_token` is a short-lived, single-use token that the data plane uses to authorize the WebSocket upgrade. The `device_public_key` is returned for TOFU verification.

#### File Transfer

```
POST /transfers/upload-url
  Auth:     Bearer <JWT>
  Request:  { device_id, filename, size, content_type }
  Response: { transfer_id, upload_url, expires_in }

POST /transfers/download-url
  Auth:     Bearer <JWT>
  Request:  { transfer_id }
  Response: { download_url, filename, size, expires_in }
```

Upload and download URLs are pre-signed S3-compatible URLs. The relay never handles file bytes.

### Data Plane Protocol

#### Connection Establishment

Both clients and daemons connect via WebSocket:

```
GET /ws?token=<connection_token>&role=<client|device>
  Upgrade: websocket
```

The `connection_token` is validated against the control plane (in-memory cache or short-lived JWT). After validation, the connection is registered in the relay's connection map.

#### Envelope Format

Every WebSocket message is a binary frame containing:

```
┌──────────────────────────────────────────────┐
│  version (1 byte)                            │
│  message_type (1 byte)                       │
│  channel_id (16 bytes, UUID)                 │
│  payload_length (4 bytes, LE)                │
│  payload (variable, opaque)                  │
└──────────────────────────────────────────────┘
```

Message types:

| Type | Value | Direction | Description |
|------|-------|-----------|-------------|
| `DATA`        | 0x01 | bidirectional | E2E encrypted rttx protocol frame |
| `CHANNEL_OPEN`   | 0x02 | client → relay | Request to open a channel to a device |
| `CHANNEL_READY`  | 0x03 | relay → both   | Channel established, both sides notified |
| `CHANNEL_CLOSE`  | 0x04 | bidirectional  | Close the channel gracefully |
| `CHANNEL_ERROR`  | 0x05 | relay → sender | Routing or authorization error |
| `PING`        | 0x06 | bidirectional | Keepalive (WebSocket ping/pong is also used) |
| `PONG`        | 0x07 | bidirectional | Keepalive response |

The relay processes `CHANNEL_OPEN`, `CHANNEL_CLOSE`, `PING`, and `PONG`. For `DATA` messages, it reads only the header (to determine `channel_id` for routing) and forwards the payload as-is.

#### Channel Lifecycle

```
Client                    Relay                     Daemon
  │                         │                         │
  │  CHANNEL_OPEN(device_id)│                         │
  │────────────────────────►│                         │
  │                         │  CHANNEL_READY          │
  │                         │────────────────────────►│
  │  CHANNEL_READY          │                         │
  │◄────────────────────────│                         │
  │                         │                         │
  │  DATA(Noise IK hello)   │  DATA(Noise IK hello)  │
  │────────────────────────►│────────────────────────►│
  │                         │                         │
  │  DATA(Noise IK resp)    │  DATA(Noise IK resp)   │
  │◄────────────────────────│◄────────────────────────│
  │                         │                         │
  │  DATA(encrypted rttx)   │  DATA(encrypted rttx)  │
  │◄───────────────────────►│◄───────────────────────►│
  │         ...             │         ...             │
  │  CHANNEL_CLOSE          │  CHANNEL_CLOSE          │
  │────────────────────────►│────────────────────────►│
```

After CHANNEL_READY, the first DATA messages carry the Noise handshake. Subsequent DATA messages carry encrypted rttx protocol frames. The relay is unaware of this distinction.

### End-to-End Encryption

**Protocol**: Noise IK handshake pattern.

The IK pattern is chosen over XX because the client already knows the daemon's public key (from device registration or a previous TOFU exchange). This saves one round trip compared to XX.

```
IK:
  <- s           (client knows daemon's static key from registration/TOFU)
  ...
  -> e, es, s, ss   (client sends ephemeral + static, encrypted)
  <- e, ee, se      (daemon responds with ephemeral)
```

After the handshake, both sides derive symmetric keys for encrypting all subsequent frames. The Noise transport messages (with padding and nonces) are placed in the DATA envelope payload.

**Key management:**
- Each daemon has a Noise static keypair, generated during `rttx-server register`
- The public key is stored in the relay control plane and returned during device discovery
- The client stores pinned keys in `~/.config/rttx/known_devices` (TOFU model)
- Key rotation: the user re-registers the device, which triggers a TOFU re-verification prompt on the client

**TOFU flow:**
1. Client fetches `device_public_key` from control plane during device discovery
2. If the key is not in `known_devices`, prompt user: "New device <name> with fingerprint <fp>. Trust this device? [Y/n]"
3. If the key differs from a pinned entry, warn: "Device <name> key has changed! This could indicate a security issue." Require explicit confirmation.
4. On acceptance, store `device_id → public_key` in `known_devices`

### Transport Adapter

The relay endpoint integrates into the existing transport abstraction. `DaemonConnection` in
`clients/rttx/src/daemon.rs` already works over any `AsyncRead + AsyncWrite` stream — it accepts
boxed trait objects and splits them into `DaemonReader`/`DaemonWriter` via `into_split()`. The
WebSocket adapter provides this interface:

```
                ┌──────────────────────────────────────┐
                │          EndpointActor                │
                │                                       │
                │  ┌─────────────┐  ┌────────────────┐ │
                │  │DaemonReader │  │ DaemonWriter   │ │
                │  └──────┬──────┘  └───────┬────────┘ │
                │         │                 │          │
                │  ┌──────┴─────────────────┴────────┐ │
                │  │     NoiseTransport              │ │
                │  │  (encrypt/decrypt + framing)    │ │
                │  └──────┬─────────────────┬────────┘ │
                │         │                 │          │
                │  ┌──────┴─────────────────┴────────┐ │
                │  │     RelayWebSocket              │ │
                │  │  (envelope + channel mgmt)      │ │
                │  └──────┬─────────────────┬────────┘ │
                │         │    read         │  write   │
                └─────────┼─────────────────┼──────────┘
                          │                 │
                       WebSocket connection to relay
```

**Layer responsibilities:**
- `RelayWebSocket`: manages the WebSocket connection, envelope serialization, channel open/close, keepalives
- `NoiseTransport`: encrypts outbound frames, decrypts inbound frames, handles Noise handshake on channel open
- `DaemonReader` / `DaemonWriter`: unchanged from the existing interface; they see plaintext rttx protocol frames

The daemon side uses the same layer stack in reverse. The `rttx-server` binary gains a `relay` subcommand (alongside `start`, `stop`, `attach-stdio`) that connects outbound to the relay and serves clients through it. This mirrors the existing `StdioStream` in `services/rttx-server/src/ipc.rs` which wraps stdin/stdout as an `AsyncRead + AsyncWrite` stream for SSH tunneling.

### Endpoint Configuration

A relay endpoint in the GUI's workspace configuration. The current `RuntimeEndpoint` enum in
`clients/rttx/src/runtime.rs` has `Local` and `Remote { host }` variants; the relay adds a
third variant:

```rust
pub enum RuntimeEndpoint {
    Local,
    Remote { host: String },
    Relay { relay_url: String, device_id: String, name: String },
}
```

Equivalent TOML representation for configuration:

```
[[endpoints]]
type = "relay"
relay_url = "wss://relay.rttx.io"    # or self-hosted URL
device_id = "a1b2c3d4-..."
name = "production-server"
```

The client stores OAuth2 credentials in the system keyring, keyed by relay URL. Device token on the daemon side is stored in `~/.config/rttx-server/relay.json`. Note: RFC-023 (client configuration state store) may change the client-side storage location; RFC-022 (daemon state storage) may change the daemon-side storage location. The relay credential storage should align with whichever storage redesign lands first.

### Connection State Machine

The relay endpoint extends the existing `ConnectionStatus` state machine (RFC-018) with
relay-specific transitions. The current implemented states are `Starting`, `Connecting`,
`Connected`, `Reconnecting`, `Blocked(ConnectionProblem)`, `Disconnected`, `Recovered`, and
`SessionMissing`. The relay adds a `Handshaking` state for the Noise key exchange:

```
                ┌──────────┐
                │ Starting │
                └────┬─────┘
                     │ initiate WebSocket + auth
                     ▼
              ┌──────────────┐
              │  Connecting  │──── WebSocket fails ───► Reconnecting
              └──────┬───────┘                              │
                     │ CHANNEL_READY                        │ backoff + retry
                     ▼                                      │
              ┌──────────────┐                              │
              │  Handshaking │──── Noise fails ────► Blocked(problem)
              └──────┬───────┘
                     │ Noise transport ready
                     ▼
              ┌──────────────┐
              │  Connected   │──── connection lost ──► Reconnecting ─┐
              └──────────────┘                              ▲        │
                                                            └────────┘
```

`Handshaking` is a new state specific to relay endpoints, covering the Noise handshake after the
relay channel is established but before rttx protocol messages can flow. Failures during
handshake transition to `Blocked(ConnectionProblem)` rather than `Disconnected`, since a Noise
failure likely indicates a key mismatch or security issue requiring user attention.

### Self-Hosted Deployment

The relay ships as a single binary: `rttx-relay`.

```
rttx-relay serve \
  --listen 0.0.0.0:443 \
  --tls-cert /path/to/cert.pem \
  --tls-key /path/to/key.pem \
  --oidc-issuer https://auth.example.com \
  --storage s3://bucket-name          # for file transfer; optional
```

In self-hosted mode, both control plane and data plane run in a single process. The binary is stateless on disk (device registry in SQLite or Postgres, configurable). For managed deployment, control plane and data plane can be split into separate scaling units.

Minimum self-hosted requirements: a single VM or container with a public IP and a TLS certificate. No additional infrastructure (Redis, message queue, etc.) is required for the base relay.

### Relay Internal Architecture

```
┌──────────────────────────────────────────────────┐
│                  rttx-relay                       │
│                                                   │
│  ┌────────────────┐      ┌─────────────────────┐ │
│  │  Control Plane │      │    Data Plane        │ │
│  │                │      │                      │ │
│  │  /auth/*       │      │  Connection Map      │ │
│  │  /devices/*    │      │  ┌────────────────┐  │ │
│  │  /transfers/*  │      │  │ channel_id →   │  │ │
│  │                │      │  │  (client_ws,   │  │ │
│  │  Device        │      │  │   device_ws)   │  │ │
│  │  Registry      │      │  └────────────────┘  │ │
│  │  (SQLite/PG)   │      │                      │ │
│  └────────────────┘      │  Frame Router        │ │
│                          │  (zero-copy fwd)     │ │
│                          └─────────────────────┘ │
└──────────────────────────────────────────────────┘
```

**Frame router**: the hot path. Reads envelope header (18 bytes: version + type + channel_id), looks up the paired connection in the connection map, writes the frame to the other side. No allocation, no payload inspection. The relay should be able to saturate network bandwidth on a modest instance.

**Connection map**: in-memory `HashMap<ChannelId, (ClientConn, DeviceConn)>`. On disconnect, the channel is removed and the other side receives `CHANNEL_CLOSE`.

**Device registry**: persistent storage for device records (id, name, public key, owner, status, last_seen). SQLite for self-hosted single-node, Postgres for managed multi-node.

## Goals Alignment

| Principle (RFC-001)              | Alignment | Status |
|----------------------------------|-----------|--------|
| P1: Native GNOME integration     | Unchanged; relay is a transport concern, not a UI concern | N/A — relay is unimplemented; existing GNOME integration is unaffected |
| P2: Rock-solid stability         | Relay is optional and additive; existing SSH and local endpoints are unaffected. Failure mode is clean: relay down = can't reach relay endpoints, everything else works | N/A — the additive design is validated by the existing `EndpointActor` architecture where each endpoint has independent lifecycle |
| P3: Workflow context over layout | Relay endpoints appear alongside local/SSH endpoints in the workspace model; no workflow changes | N/A — `RuntimeEndpoint` already distinguishes endpoint types; adding a relay variant preserves the model |
| P4: Composable building blocks   | The relay is a pure transport primitive. E2E encryption, auth, and frame routing are independent layers that compose | N/A — the layered transport adapter design (RelayWebSocket → NoiseTransport → DaemonReader/Writer) follows the existing `DaemonConnection` abstraction |
| P5: Practical tools              | Solves a real pain point (accessing hosts behind NAT/firewalls) without requiring VPN or bastion infrastructure | N/A — unimplemented |

## Development Plan

All phases are unimplemented. The prerequisite infrastructure (monorepo, endpoint actor
architecture, transport abstraction, connection state machine) is complete on `mainline`.

### Phase 1: WebSocket Transport Adapter
- [ ] Implement `RelayWebSocket` layer: WebSocket client with envelope serialization/deserialization
- [ ] Implement `RelayWebSocket` on daemon side with outbound connection and reconnection
- [ ] Add `Relay { device_id }` variant to `RuntimeEndpoint` and wire it through `EndpointConnectionManager`
- [ ] Add `Handshaking` state to `ConnectionStatus` (extends RFC-018 state machine)
- [ ] Integration test: client ↔ in-process relay ↔ daemon, plaintext (no encryption), verify rttx protocol frames pass through

*Prerequisite*: the `DaemonConnection` abstraction (#136, implemented) and `EndpointActor`
architecture already support pluggable transports. Protocol v3 (RFC-021) may affect the
handshake and capability negotiation if it lands first.

### Phase 2: Relay Service (Data Plane)
- [ ] Implement `rttx-relay` binary with WebSocket listener
- [ ] Connection map and frame router (zero-copy forwarding)
- [ ] Channel lifecycle: CHANNEL_OPEN, CHANNEL_READY, CHANNEL_CLOSE, CHANNEL_ERROR
- [ ] Keepalive (PING/PONG) with configurable interval and timeout
- [ ] Integration test: client ↔ relay process ↔ daemon over network

### Phase 3: Authentication & Device Registration
- [ ] Control plane REST API: /auth/*, /devices/*
- [ ] OAuth2/OIDC client integration (authorization code flow + device code flow for headless)
- [ ] Device registration CLI: `rttx-server register --relay <url>`
- [ ] Device token storage and refresh on daemon side
- [ ] JWT-based connection token issuance for data plane WebSocket upgrade
- [ ] Device list UI in rttx client
- [ ] Device registry storage (SQLite for self-hosted)

### Phase 4: End-to-End Encryption
- [ ] Noise IK handshake implementation (using `snow` crate)
- [ ] `NoiseTransport` layer: encrypt/decrypt rttx frames over the relay channel
- [ ] Keypair generation during device registration
- [ ] TOFU key pinning in `~/.config/rttx/known_devices`
- [ ] Key mismatch warning UI
- [ ] Integration test: verify relay cannot read forwarded payloads

### Phase 5: File Transfer
- [ ] Control plane API: /transfers/*
- [ ] Pre-signed URL generation for S3-compatible storage
- [ ] File upload from daemon, brokered via E2E encrypted metadata messages
- [ ] File download in client via pre-signed URL
- [ ] Transfer progress UI in rttx

### Phase 6: Production Readiness
- [ ] TLS termination and certificate configuration
- [ ] Rate limiting and connection limits on both planes
- [ ] Metrics and observability (connection count, frame throughput, latency percentiles)
- [ ] Managed deployment configuration (split control/data planes, Postgres backend)
- [ ] Documentation: self-hosted deployment guide, security model overview

**Dependencies**: Phase 1 → Phase 2 → Phase 3 → Phase 4. Phase 5 depends on Phase 3. Phase 6 is parallel to Phase 4/5.

**Interactions with other RFCs**: Protocol v3 (RFC-021) may change the handshake and framing
format. Daemon state storage v2 (RFC-022) may affect how device registration credentials are
persisted on the daemon side. Client configuration store (RFC-023) may affect where relay
endpoint configuration and TOFU keys are stored on the client side.

## Open Questions

- [ ] Q1: **Noise IK vs. XX**: IK saves a round trip but requires the client to know the daemon's public key before the handshake. If the control plane is compromised and serves a wrong key, the client would connect to an impersonator. Is TOFU verification sufficient, or should we support an out-of-band key verification mechanism (QR code, fingerprint comparison)? *Progress*: the TOFU model is consistent with SSH's `known_hosts` approach, which rttx users are already familiar with. An out-of-band verification option (fingerprint display on daemon, manual comparison on client) is low-cost to add alongside TOFU and would strengthen the security story for high-value hosts.
- [ ] Q2: **Multi-region routing**: for the managed relay, should the data plane support regional endpoints with automatic routing (client connects to nearest region, relay routes to daemon's region)? Or is single-region sufficient for v1? *Progress*: single-region is sufficient for v1 and self-hosted deployments. Multi-region can be added later as a managed-service optimization without protocol changes — the client already discovers the data plane URL from the control plane, so regional routing is a control plane concern.
- [ ] Q3: **Device sharing**: should a user be able to share a device with another user (granting relay access to someone else's daemon)? This has significant UX and security implications. Likely a post-v1 concern but worth considering in the authorization model. *Progress*: the daemon's single-writer model (RFC-013) means sharing a device grants terminal access to all runtimes on that host. This is a significant security decision that should be deferred to post-v1 but the authorization model should not preclude it.
- [ ] Q4: **Connection multiplexing**: should a single WebSocket tunnel carry multiple channels (to different devices), or one tunnel per channel? Multiplexing is more efficient but adds complexity to the envelope routing. *Progress*: the envelope format already includes a `channel_id` field, so multiplexing is supported at the wire level. The initial implementation can use one tunnel per channel for simplicity and add multiplexing as an optimization later.
- [ ] Q5: **Relay protocol versioning**: the envelope format includes a version byte. What is the compatibility contract? Must the relay support older envelope versions, or can it require clients/daemons to upgrade? *Progress*: RFC-021 (protocol v3) introduces capability negotiation for the rttx protocol. The relay envelope version should follow a similar pattern: the relay advertises supported versions during WebSocket upgrade, and clients/daemons select the highest common version.
- [ ] Q6: **Daemon auto-registration**: should `rttx-server` support a mode where it automatically registers with a pre-configured relay on first startup (using a provisioning token), for fleet deployment scenarios? *Progress*: this is a post-v1 concern. The initial implementation should require explicit `rttx-server register --relay <url>` invocation. Auto-registration can be added later with a provisioning token mechanism.

## References

- [RFC-001: Project Manifesto](RFC-001-manifesto.md)
- [RFC-010: Maintainability Refactor](RFC-010-maintainability-refactor.md)
- [RFC-013: Daemon-Backed Workspaces and Runtimes](RFC-013-persistent-host-sessions.md)
- [RFC-016: Workspace Management v2](RFC-016-workspace-management-v2.md) — action-oriented workspace creation model
- [RFC-018: Workspace Connection State Machine](RFC-018-workspace-connection-state-machine.md) — the state machine this RFC extends with `Handshaking`
- [RFC-019: Missing Session Handling](RFC-019-missing-session-handling.md) — `SessionMissing` state for disappeared runtimes
- [RFC-020: Terminal Response Ownership](RFC-020-terminal-response-ownership.md) — daemon answers terminal queries independently of client
- [RFC-021: Client/Server Protocol v3](RFC-021-client-server-protocol-v3.md) — protocol evolution that may affect relay handshake and framing
- [RFC-022: Daemon State Storage](RFC-022-daemon-state-storage.md) — may affect device credential persistence on daemon side
- [RFC-023: Client Configuration State Store](RFC-023-client-configuration-state-store.md) — may affect relay endpoint and TOFU key storage on client side
- [Noise Protocol Framework](https://noiseprotocol.org/noise.html)
- [Noise IK Pattern](https://noiseprotocol.org/noise.html#interactive-handshake-patterns-fundamental)
- [VS Code Remote Tunnels](https://code.visualstudio.com/docs/remote/tunnels) — prior art for relay-based remote development
- [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) — prior art for outbound-only connectivity
- [Tracking issue](https://github.com/IllyaYalovyy/rttx/issues/604)
