# Rust Networking Protocol Crate Recommendation

## Summary

There does not appear to be a single mature, small, pure-Rust crate that supports all of the following protocols:

- HTTP
- FTP
- SMTP
- POP
- NNTP
- DNS
- TCP
- UDP
- WHOIS
- Finger
- Daytime

The closest broad multiprotocol option is the Rust `curl` crate, which wraps `libcurl`. It is mature and supports many protocols, including HTTP, FTP, SMTP, and POP3, but it is not pure Rust and does not cover every protocol in the list as a first-class feature.

For a modern Rust project, especially one inspired by REBOL/Red-style networking, the better recommendation is to build a small protocol facade over a curated set of crates and direct socket implementations.

---

## Recommended Approach

Use a **composed protocol layer** rather than looking for one crate to handle everything.

The design should have:

1. A uniform public API for all supported protocols.
2. Mature crates underneath where they make sense.
3. Direct `std::net` or async socket code for simple legacy protocols.
4. Optional async support later, instead of forcing async from the beginning.

This gives you the ergonomics of a REBOL-like networking surface without depending on a single large or incomplete dependency.

# Compose Idiomatic Rust Crates

For a more idiomatic and maintainable Rust design, compose protocol-specific crates.

### Suggested Dependency Set

```toml
[dependencies]
ureq = "3"              # Small blocking HTTP client
suppaftp = "6"          # FTP client
lettre = "0.11"         # SMTP/email sending
domain = "0.11"         # DNS protocol building blocks
```

Optional niche protocol crates:

```toml
[dependencies]
rust-pop3-client = "0.3" # POP3 client
nntp-rs = "0.1"          # NNTP client
```

Use direct socket code for simple protocols:

- TCP
- UDP
- WHOIS
- Finger
- Daytime

---

## Protocol-by-Protocol Recommendation

| Protocol | Recommended Implementation |
|---|---|
| HTTP | `ureq`, `reqwest`, `hyper`, or `curl` |
| FTP | `suppaftp` or `curl` |
| SMTP | `lettre` or `curl` |
| POP3 | `curl` or `rust-pop3-client` |
| NNTP | `nntp-rs` or direct TCP implementation |
| DNS | `domain`, `hickory-dns`, or `dns-lookup` |
| TCP | `std::net::TcpStream` / `TcpListener` |
| UDP | `std::net::UdpSocket` |
| WHOIS | Direct TCP on port 43 |
| Finger | Direct TCP on port 79 |
| Daytime | Direct TCP or UDP on port 13 |

---

## Why Direct Socket Code Is Reasonable for Some Protocols

Some of the older protocols are extremely small. A full crate may not be necessary.

### WHOIS

WHOIS is a simple TCP protocol, usually on port 43.

A client connects to the server, sends a query followed by CRLF, then reads the response until the server closes the connection.

Conceptually:

```text
connect whois.example.net:43
send "example.com\r\n"
read response
```

### Finger

Finger is similarly simple. It commonly uses TCP port 79.

A client sends a one-line query and receives a text response.

Conceptually:

```text
connect host:79
send "user\r\n"
read response
```

### Daytime

Daytime uses TCP or UDP port 13.

The server returns a human-readable ASCII date/time string.

Conceptually:

```text
connect host:13
read response
```

Because these protocols are so small, implementing them directly gives you better control and avoids unnecessary dependencies.

---

## Recommended Facade Design

Instead of exposing each crate directly, create your own protocol facade.

For example:

```rust
enum Protocol {
    Http,
    Ftp,
    Smtp,
    Pop3,
    Nntp,
    Dns,
    Tcp,
    Udp,
    Whois,
    Finger,
    Daytime,
}
```

Then define a common request/response model.

```rust
struct NetworkRequest {
    protocol: Protocol,
    target: String,
    port: Option<u16>,
    payload: Option<Vec<u8>>,
    options: NetworkOptions,
}

struct NetworkResponse {
    status: NetworkStatus,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct NetworkOptions {
    timeout_ms: Option<u64>,
    tls: bool,
    follow_redirects: bool,
}

enum NetworkStatus {
    Ok,
    ProtocolStatus(u16),
    Error(String),
}
```

This allows the higher-level language or runtime to expose one consistent networking model while still using specialized Rust implementations internally.

---

## Example Module Layout

```text
src/
  net/
    mod.rs
    request.rs
    response.rs
    protocol.rs
    error.rs

    http.rs
    ftp.rs
    smtp.rs
    pop3.rs
    nntp.rs
    dns.rs

    tcp.rs
    udp.rs
    whois.rs
    finger.rs
    daytime.rs
```

The public API would live in `net/mod.rs`, while individual protocol details stay isolated in separate modules.

---

## Example Public API

A REBOL/Red-inspired Rust API might look like this:

```rust
let response = net::open("http://example.com")?;
let response = net::open("ftp://ftp.example.com/file.txt")?;
let response = net::open("whois://example.com")?;
let response = net::open("finger://user@example.com")?;
let response = net::open("daytime://time.example.com")?;
```

Or with a more explicit builder:

```rust
let response = NetworkRequest::new(Protocol::Whois)
    .target("example.com")
    .send()?;
```

For TCP and UDP:

```rust
let response = NetworkRequest::new(Protocol::Tcp)
    .target("example.com")
    .port(1234)
    .payload(b"hello".to_vec())
    .send()?;
```

---

## Blocking First, Async Later

For a small runtime or REBOL-like system, blocking I/O is a good starting point.

Advantages:

- Simpler implementation.
- Easier embedding.
- Fewer dependency constraints.
- Easier to expose to a dynamic language runtime.
- Works well for scripting and automation use cases.

Later, async support can be added behind a feature flag:

```toml
[features]
default = ["blocking"]
blocking = []
async = ["tokio"]
```

The facade can then support both modes internally.

---

## Recommended Dependency Strategy

Start with the minimum viable dependency set:

```toml
[dependencies]
ureq = "3"
suppaftp = "6"
lettre = "0.11"
domain = "0.11"
```

Implement directly:

- TCP
- UDP
- WHOIS
- Finger
- Daytime

Defer until needed:

- POP3
- NNTP
- async support
- TLS customization
- proxy support
- SOCKS support
- WebSocket support

---

## Final Recommendation

For a small, mature Rust networking layer, do **not** search for one crate that supports everything.

Instead:

1. Use `ureq` for HTTP.
2. Use `suppaftp` for FTP.
3. Use `lettre` for SMTP.
4. Use `domain` or `hickory-dns` for DNS.
5. Use `std::net` directly for TCP, UDP, WHOIS, Finger, and Daytime.
6. Add POP3 and NNTP only if they become real requirements.
7. Wrap everything in your own protocol facade.

This design gives you a small, mature, idiomatic Rust foundation while preserving the ability to expose a simple, uniform, REBOL-like networking experience.
