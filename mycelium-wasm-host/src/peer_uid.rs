//! **Who is on the other end of a loopback connection** (#644's round-3 review): the user id of the process
//! that owns the client socket of a TCP connection to this host, read from the kernel, so an activation's egress
//! proxy can refuse at accept every connection that is not the stem's own user's — another user on the host
//! cannot hold a place in the proxy's accounting, token or not.
//!
//! - **Linux:** a `sock_diag` (`NETLINK_SOCK_DIAG`, `SOCK_DIAG_BY_FAMILY`) query for the exact 4-tuple — the
//!   kernel answers it from its connection hash, so its cost does not grow with the host's TCP table, which
//!   another user can inflate (#644's round-7 review) — its `idiag_uid`, the socket in a live state
//!   ([`LIVE_STATES`]). Built and parsed as plain bytes over `rustix`'s netlink socket, no `unsafe`. Where a
//!   netlink socket cannot be opened (a sandbox that forbids it), `/proc/net/tcp` (and `tcp6`) instead: the live
//!   row whose local address is the peer's and whose remote address is ours, its `uid` column — still
//!   fail-closed, but a read of the whole table.
//! - **macOS:** `sysctl -b net.inet.tcp.pcblist_n` (run as `/usr/sbin/sysctl`, so no `unsafe` call): the
//!   `XSO_INPCB` record whose addresses and ports are the peer's and ours, then the `XSO_SOCKET` record that
//!   follows it in the same group — checked to be a TCP stream, its `so_uid` (offset 64 of `struct xsocket_n`,
//!   `#pragma pack(4)` in xnu's `bsd/netinet/in_pcb.h` — confirmed by
//!   `the_kernel_names_this_users_own_loopback_connection`; a layout this parser does not know matches nothing).
//!
//! One read answers a whole batch of connections ([`loopback_peer_uids`]).
//!
//! Anything that cannot be read or matched is an `Err` — the caller **fails closed**. Elsewhere no lookup exists,
//! and every connection is refused. Like the proxy's sockets, these reads are outside the replay seams.

use std::net::SocketAddr;

/// How long `sysctl` may run before it is killed (the proxy's own read bound is the backstop).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SYSCTL_TIMEOUT_MS: u64 = 1_500;

/// `sock_diag`: one request for one socket by its exact addresses, and its answer — plain bytes, native-endian
/// headers and network-order ports and addresses, per `linux/netlink.h`, `linux/sock_diag.h` and
/// `linux/inet_diag.h` (`struct nlmsghdr`, `struct inet_diag_req_v2`, `struct inet_diag_msg`).
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
mod diag {
    use std::net::{IpAddr, SocketAddr};

    pub(super) const SOCK_DIAG_BY_FAMILY: u16 = 20;
    const NLMSG_ERROR: u16 = 2;
    const NLM_F_REQUEST: u16 = 1;
    const IPPROTO_TCP: u8 = 6;
    const AF_INET: u8 = 2;
    const AF_INET6: u8 = 10;
    /// `nlmsghdr` (16) + `inet_diag_req_v2` (56).
    pub(super) const REQUEST_LEN: usize = 72;
    /// Where `inet_diag_msg`'s fields sit after the 16-byte header.
    const MSG: usize = 16;

    /// The 48-byte `inet_diag_sockid` of the socket whose local address is `local` and remote `remote`, cookie
    /// `INET_DIAG_NOCOOKIE`. An IPv4 pair is asked for as `AF_INET` — the kernel's IPv4 lookup also finds a
    /// dual-stack socket reaching the IPv4 listener through `::ffff:…`.
    fn sockid(local: SocketAddr, remote: SocketAddr, cookie: bool) -> (u8, Vec<u8>) {
        sockid_as(local, remote, cookie, false)
    }

    /// [`sockid`], or with `as_v6` the IPv6 spelling (an IPv4 pair as `::ffff:…`).
    fn sockid_as(local: SocketAddr, remote: SocketAddr, cookie: bool, as_v6: bool) -> (u8, Vec<u8>) {
        let mut id = Vec::with_capacity(48);
        id.extend_from_slice(&local.port().to_be_bytes());
        id.extend_from_slice(&remote.port().to_be_bytes());
        let v4 = |a: IpAddr| match a {
            IpAddr::V4(v) => Some(v),
            IpAddr::V6(v) => v.to_ipv4_mapped(),
        };
        let family = match (v4(local.ip()), v4(remote.ip())) {
            (Some(l), Some(r)) if !as_v6 => {
                for a in [l, r] {
                    id.extend_from_slice(&a.octets());
                    id.extend_from_slice(&[0; 12]);
                }
                AF_INET
            }
            _ => {
                for a in [local.ip(), remote.ip()] {
                    let v6 = match a {
                        IpAddr::V4(v) => v.to_ipv6_mapped(),
                        IpAddr::V6(v) => v,
                    };
                    id.extend_from_slice(&v6.octets());
                }
                AF_INET6
            }
        };
        id.extend_from_slice(&0u32.to_ne_bytes()); // idiag_if: any
        if cookie {
            id.extend_from_slice(&[0xff; 8]); // INET_DIAG_NOCOOKIE
        }
        (family, id)
    }

    /// The request for the TCP socket whose local address is `local` and remote `remote` (the client's socket:
    /// its local address is the peer's, its remote ours), sequence `seq`. Not a dump: one socket, looked up.
    pub(super) fn request(seq: u32, local: SocketAddr, remote: SocketAddr) -> Vec<u8> {
        let (family, id) = sockid(local, remote, true);
        let mut b = Vec::with_capacity(REQUEST_LEN);
        b.extend_from_slice(&(REQUEST_LEN as u32).to_ne_bytes());
        b.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        b.extend_from_slice(&NLM_F_REQUEST.to_ne_bytes());
        b.extend_from_slice(&seq.to_ne_bytes());
        b.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid: the kernel
        b.extend_from_slice(&[family, IPPROTO_TCP, 0, 0]); // family, protocol, ext, pad
        b.extend_from_slice(&u32::MAX.to_ne_bytes()); // idiag_states: any
        b.extend_from_slice(&id);
        debug_assert_eq!(b.len(), REQUEST_LEN);
        b
    }

    /// The answer in `buf` to request `seq` for `local` → `remote`: `Ok((state, uid))`, or why not — the kernel's
    /// error (`ENOENT`: no such socket), another message, or a socket whose addresses are not the ones asked for.
    pub(super) fn parse(buf: &[u8], seq: u32, local: SocketAddr, remote: SocketAddr) -> Result<(u8, u32), String> {
        let u16_at = |o: usize| buf.get(o..o + 2).map(|b| u16::from_ne_bytes([b[0], b[1]]));
        let u32_at = |o: usize| buf.get(o..o + 4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
        let (Some(len), Some(kind), Some(got_seq)) = (u32_at(0), u16_at(4), u32_at(8)) else {
            return Err("a sock_diag answer shorter than its header".into());
        };
        if got_seq != seq || (len as usize) > buf.len() {
            return Err("a sock_diag answer to another request".into());
        }
        if kind == NLMSG_ERROR {
            let errno = u32_at(MSG).map(|e| -(e as i32)).unwrap_or(0);
            return Err(if errno == 2 { "no such connection (sock_diag: ENOENT)".into() } else { format!("sock_diag error {errno}") });
        }
        if kind != SOCK_DIAG_BY_FAMILY || (len as usize) < MSG + 72 {
            return Err(format!("an unexpected sock_diag answer (type {kind}, {len} bytes)"));
        }
        // The ports and addresses (36 bytes) must be the ones asked for — in the IPv4 spelling, or, for a dual-stack
        // socket the IPv4 lookup found, the IPv4-mapped IPv6 one the kernel describes it in.
        let same = |as_v6: bool| {
            let (family, id) = sockid_as(local, remote, false, as_v6);
            buf[MSG] == family && buf.get(MSG + 4..MSG + 40) == Some(&id[..36])
        };
        if !same(false) && !same(true) {
            return Err("sock_diag described another socket".into());
        }
        let state = buf[MSG + 1];
        let uid = u32_at(MSG + 64).ok_or("a sock_diag answer without a uid")?;
        Ok((state, uid))
    }
}

/// The `TCP_*` states (`include/net/tcp_states.h`) of a client socket whose connection is still open on its
/// side — the numeric twins of [`LIVE_STATES`].
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LIVE_TCP_STATES: &[u8] = &[1, 4, 5, 8];

/// How long one `sock_diag` answer may take.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const DIAG_TIMEOUT_MS: u64 = 500;

/// This process's real user id.
pub fn own_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// The uid owning the client socket at `peer` connected to our `local` socket.
pub fn loopback_peer_uid(local: SocketAddr, peer: SocketAddr) -> Result<u32, String> {
    loopback_peer_uids(&[(local, peer)]).pop().unwrap_or_else(|| Err("no answer".into()))
}

/// Every `(our local, the peer)` pair's owner from **one** read of the kernel's table (#644's round-4 review:
/// a lookup's cost grows with the sockets open, so a flood must not multiply it — the proxy batches every
/// connection accepted before a read begins into that read).
pub fn loopback_peer_uids(pairs: &[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> {
    os::lookup_all(pairs)
}

/// `/proc/net/tcp{,6}`'s spelling of an address: the address's bytes as the kernel's host-order words, `%08X`
/// each, then `:%04X` of the port.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn proc_spelling(a: SocketAddr) -> String {
    let addr = match a.ip() {
        std::net::IpAddr::V4(v4) => format!("{:08X}", u32::from_ne_bytes(v4.octets())),
        std::net::IpAddr::V6(v6) => v6.octets().chunks(4).map(|w| format!("{:08X}", u32::from_ne_bytes([w[0], w[1], w[2], w[3]]))).collect(),
    };
    format!("{addr}:{:04X}", a.port())
}

/// The `/proc/net/tcp{,6}` states of a client socket whose connection is still open on its side: ESTABLISHED
/// (`01`), FIN_WAIT1 (`04`), FIN_WAIT2 (`05`) and CLOSE_WAIT (`08`) — a client may half-close before the read
/// (#644's round-5 review, finding 4). Never `TIME_WAIT` (`06`): an earlier connection's row, uid 0 (round 4).
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
const LIVE_STATES: &[&str] = &["01", "04", "05", "08"];

/// The uid on the live `/proc/net/tcp{,6}` row whose local address is `peer` and remote address `local`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_proc_net_tcp(text: &str, local: SocketAddr, peer: SocketAddr) -> Option<u32> {
    let (want_local, want_remote) = (proc_spelling(peer), proc_spelling(local));
    text.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        (f.len() > 7 && LIVE_STATES.contains(&f[3]) && f[1].eq_ignore_ascii_case(&want_local) && f[2].eq_ignore_ascii_case(&want_remote))
            .then(|| f[7].parse::<u32>().ok())
            .flatten()
    })
}

/// The owner of `peer` → `local` from the Linux tables: an IPv4 pair in `/proc/net/tcp`, or — a dual-stack client
/// that reached the IPv4 listener through `::ffff:127.0.0.1` — under the IPv4-mapped spelling in `/proc/net/tcp6`
/// (#644's round-5 review, finding 1: the JVM's default); an IPv6 pair in `/proc/net/tcp6`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn lookup_in_tables(tcp: Option<&str>, tcp6: Option<&str>, local: SocketAddr, peer: SocketAddr) -> Option<u32> {
    let mapped = |a: SocketAddr| match a {
        SocketAddr::V4(v4) => SocketAddr::new(std::net::IpAddr::V6(v4.ip().to_ipv6_mapped()), v4.port()),
        v6 => v6,
    };
    match peer {
        SocketAddr::V4(_) => tcp
            .and_then(|t| parse_proc_net_tcp(t, local, peer))
            .or_else(|| tcp6.and_then(|t| parse_proc_net_tcp(t, mapped(local), mapped(peer)))),
        SocketAddr::V6(_) => tcp6.and_then(|t| parse_proc_net_tcp(t, local, peer)),
    }
}

/// Run `program args`, its standard output read as it is written, killing it past `timeout_ms` (#644's round-5
/// review, finding 3: a hung `sysctl` must not stall the proxy). `Err` names why.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn run_bounded(program: &str, args: &[&str], timeout_ms: u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let deadline = mycelium::sim_seam::mono_now_ns().saturating_add(timeout_ms.saturating_mul(1_000_000));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if mycelium::sim_seam::mono_now_ns() < deadline => std::thread::sleep(std::time::Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("{program} did not finish within {timeout_ms} ms — killed"));
            }
        }
    };
    let bytes = reader.join().map_err(|_| format!("{program}: its output reader panicked"))?;
    if status.success() { Ok(bytes) } else { Err(format!("{program} exited {status}")) }
}

/// One TCP connection in a `net.inet.tcp.pcblist_n` buffer: (local address, local port, foreign address,
/// foreign port, owner's uid).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
type PcbRow = (std::net::IpAddr, u16, std::net::IpAddr, u16, u32);

/// Every connection in a `net.inet.tcp.pcblist_n` buffer — each `XSO_INPCB` record (ports at 16 and 18 in network
/// order; `inp_vflag` at 44; the foreign address in the union at 48 and the local at 64, an IPv4 address in the
/// union's last four bytes) with the `XSO_SOCKET` record that follows it in its group (`so_type` at 16,
/// `xso_protocol` at 36, `so_uid` at 64; `#pragma pack(4)` in xnu). A socket record that is not a TCP stream
/// (`so_type` 1, protocol 6) is a layout this parser does not know, and its connection is left out — the caller
/// then finds no match and fails closed (#644's round-4 review, finding 6).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn parse_pcblist_n(data: &[u8]) -> Vec<PcbRow> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    const XSO_SOCKET: u32 = 0x001;
    const XSO_INPCB: u32 = 0x010;
    const INP_IPV4: u8 = 0x1;
    const INP_IPV6: u8 = 0x2;
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let i16_at = |o: usize| data.get(o..o + 2).map(|b| i16::from_le_bytes([b[0], b[1]]));
    let be16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let addr_at = |o: usize, vflag: u8| -> Option<IpAddr> {
        if vflag & INP_IPV4 != 0 {
            let b = data.get(o + 12..o + 16)?;
            Some(IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3])))
        } else if vflag & INP_IPV6 != 0 {
            let b: [u8; 16] = data.get(o..o + 16)?.try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(b)))
        } else {
            None
        }
    };
    let mut rows = Vec::new();
    let Some(first) = u32_at(0) else { return rows };
    let mut off = first as usize; // the leading `struct xinpgen`
    let mut pending: Option<(IpAddr, u16, IpAddr, u16)> = None;
    while off + 8 <= data.len() {
        let (Some(len), Some(kind)) = (u32_at(off), u32_at(off + 4)) else { break };
        let len = len as usize;
        if len < 8 {
            break;
        }
        if kind == XSO_INPCB {
            pending = (|| {
                let vflag = *data.get(off + 44)?;
                Some((addr_at(off + 64, vflag)?, be16_at(off + 18)?, addr_at(off + 48, vflag)?, be16_at(off + 16)?))
            })();
        } else if kind == XSO_SOCKET
            && let Some((la, lp, fa, fp)) = pending.take()
        {
            let stream_tcp = i16_at(off + 16) == Some(1) && u32_at(off + 36) == Some(6);
            if let (true, true, Some(uid)) = (stream_tcp, len >= 68, u32_at(off + 64)) {
                rows.push((la, lp, fa, fp, uid));
            }
        }
        off += (len + 7) & !7;
    }
    rows
}

/// The owner of the client socket at `peer` connected to `local`, among `rows`: local and foreign addresses
/// **and** ports matched (#644's round-4 review, finding 2).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn match_pcb(rows: &[PcbRow], local: SocketAddr, peer: SocketAddr) -> Option<u32> {
    // An IPv4-mapped address and its IPv4 address are one address (a dual-stack client, round 5).
    let same = |a: &std::net::IpAddr, b: std::net::IpAddr| a.to_canonical() == b.to_canonical();
    rows.iter()
        .find(|(la, lp, fa, fp, _)| same(la, peer.ip()) && *lp == peer.port() && same(fa, local.ip()) && *fp == local.port())
        .map(|r| r.4)
}

#[cfg(target_os = "linux")]
mod os {
    use rustix::net::{netlink, AddressFamily, RecvFlags, SendFlags, SocketType};
    use std::net::SocketAddr;
    use std::os::fd::OwnedFd;

    pub(super) fn lookup_all(pairs: &[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> {
        match open() {
            Ok(fd) => pairs.iter().enumerate().map(|(i, (local, peer))| one(&fd, i as u32 + 1, *local, *peer)).collect(),
            // No netlink here (a sandbox that forbids it): the whole-table read, still fail-closed.
            Err(e) => {
                tracing::debug!(error = %e, "peer uid: no sock_diag socket; reading /proc/net/tcp");
                proc_tables(pairs)
            }
        }
    }

    fn open() -> rustix::io::Result<OwnedFd> {
        let fd = rustix::net::socket(AddressFamily::NETLINK, SocketType::DGRAM, Some(netlink::SOCK_DIAG))?;
        rustix::net::sockopt::set_socket_timeout(&fd, rustix::net::sockopt::Timeout::Recv, Some(std::time::Duration::from_millis(super::DIAG_TIMEOUT_MS)))?;
        Ok(fd)
    }

    /// One socket by its exact addresses: the client's socket, whose local address is `peer` and remote `local`.
    pub(super) fn one(fd: &OwnedFd, seq: u32, local: SocketAddr, peer: SocketAddr) -> Result<u32, String> {
        let req = super::diag::request(seq, peer, local);
        rustix::net::sendto(fd, &req, SendFlags::empty(), &netlink::SocketAddrNetlink::new(0, 0)).map_err(|e| format!("sock_diag send: {e}"))?;
        let mut buf = [0u8; 8192];
        // Skip a stale answer to an earlier request on this socket (one that timed out), at most a few.
        for _ in 0..8 {
            let (n, _) = rustix::net::recv(fd, &mut buf[..], RecvFlags::empty()).map_err(|e| format!("sock_diag receive: {e}"))?;
            match super::diag::parse(&buf[..n], seq, peer, local) {
                Err(e) if e.contains("another request") => continue,
                Err(e) => return Err(format!("{peer} → {local}: {e}")),
                Ok((state, uid)) if super::LIVE_TCP_STATES.contains(&state) => return Ok(uid),
                Ok((state, _)) => return Err(format!("{peer} → {local} is not a live connection (TCP state {state})")),
            }
        }
        Err("sock_diag: no answer to this request".into())
    }

    /// The fallback: `/proc/net/tcp` and `tcp6`, read whole.
    pub(super) fn proc_tables(pairs: &[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> {
        let read = |table: &str| std::fs::read_to_string(table).ok();
        // tcp6 also holds a dual-stack client of the IPv4 listener, under the IPv4-mapped spelling.
        let tcp = pairs.iter().any(|(_, p)| p.is_ipv4()).then(|| read("/proc/net/tcp")).flatten();
        let tcp6 = read("/proc/net/tcp6");
        pairs
            .iter()
            .map(|(local, peer)| {
                super::lookup_in_tables(tcp.as_deref(), tcp6.as_deref(), *local, *peer)
                    .ok_or_else(|| format!("{peer} → {local} is not a live connection in /proc/net/tcp or tcp6"))
            })
            .collect()
    }

    /// For the tests: the sock_diag path alone, no fallback.
    #[cfg(test)]
    pub(super) fn diag_only(pairs: &[(SocketAddr, SocketAddr)]) -> Result<Vec<Result<u32, String>>, String> {
        let fd = open().map_err(|e| e.to_string())?;
        Ok(pairs.iter().enumerate().map(|(i, (local, peer))| one(&fd, i as u32 + 1, *local, *peer)).collect())
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::net::SocketAddr;

    pub(super) fn lookup_all(pairs: &[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> {
        let rows = super::run_bounded("/usr/sbin/sysctl", &["-b", "net.inet.tcp.pcblist_n"], super::SYSCTL_TIMEOUT_MS)
            .map(|out| super::parse_pcblist_n(&out));
        pairs
            .iter()
            .map(|(local, peer)| match &rows {
                Ok(rows) => super::match_pcb(rows, *local, *peer).ok_or_else(|| format!("{peer} → {local} is not in net.inet.tcp.pcblist_n")),
                Err(e) => Err(e.clone()),
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod os {
    use std::net::SocketAddr;

    pub(super) fn lookup_all(pairs: &[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> {
        pairs.iter().map(|_| Err(format!("no loopback peer-uid lookup on {}", std::env::consts::OS))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel names this process's own user for a loopback connection this process made — the lookup the
    /// proxy runs at accept, on a live connection, on this platform.
    #[test]
    fn the_kernel_names_this_users_own_loopback_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(local).unwrap();
        let (_server, peer) = listener.accept().unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
        assert_eq!(loopback_peer_uid(local, peer), Ok(own_uid()));
        // A connection that does not exist is an error, never a uid.
        let gone: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert!(loopback_peer_uid(local, gone).is_err());
    }

    /// A synthetic `pcblist_n` group: an `XSO_INPCB` record for local `l` and foreign `f`, then an `XSO_SOCKET`
    /// record of type `so_type` owned by `uid`.
    fn pcb_group(l: SocketAddr, f: SocketAddr, so_type: i16, uid: u32) -> Vec<u8> {
        let record = |kind: u32| {
            let mut r = vec![0u8; 104];
            r[0..4].copy_from_slice(&104u32.to_le_bytes());
            r[4..8].copy_from_slice(&kind.to_le_bytes());
            r
        };
        let v4 = |a: SocketAddr| match a.ip() {
            std::net::IpAddr::V4(v) => v.octets(),
            _ => unreachable!(),
        };
        let mut inpcb = record(0x010);
        inpcb[16..18].copy_from_slice(&f.port().to_be_bytes());
        inpcb[18..20].copy_from_slice(&l.port().to_be_bytes());
        inpcb[44] = 0x1;
        inpcb[60..64].copy_from_slice(&v4(f));
        inpcb[76..80].copy_from_slice(&v4(l));
        let mut socket = record(0x001);
        socket[16..18].copy_from_slice(&so_type.to_le_bytes());
        socket[36..40].copy_from_slice(&6u32.to_le_bytes());
        socket[64..68].copy_from_slice(&uid.to_le_bytes());
        [inpcb, socket, record(0x002)].concat()
    }

    /// The macOS parser on a synthetic buffer (the live test shows the real layout): the socket record after
    /// the matching inpcb gives the uid — matched on both addresses and both ports, so a connection with the
    /// same ports between other addresses is not taken (#644's round-4 review, finding 2; seen failing first with
    /// ports only: the first group's uid, 7, was returned); a socket record that is not a TCP stream is a layout
    /// not understood and matches nothing (finding 6; seen failing first: its uid was returned).
    #[test]
    fn a_pcblist_n_group_is_matched_on_both_addresses_and_ports() {
        let ours: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let elsewhere_l: SocketAddr = "127.0.0.2:40000".parse().unwrap();
        let elsewhere_f: SocketAddr = "127.0.0.3:8080".parse().unwrap();
        let mut buf = vec![0u8; 24];
        buf[0..4].copy_from_slice(&24u32.to_le_bytes());
        buf.extend(pcb_group(elsewhere_l, elsewhere_f, 1, 7));
        buf.extend(pcb_group(peer, ours, 1, 501));
        let rows = parse_pcblist_n(&buf);
        assert_eq!(match_pcb(&rows, ours, peer), Some(501), "{rows:?}");
        assert_eq!(match_pcb(&rows, ours, "127.0.0.1:40002".parse().unwrap()), None);
        assert!(parse_pcblist_n(&buf[..30]).is_empty(), "a truncated buffer is no answer");
        let mut odd = vec![0u8; 24];
        odd[0..4].copy_from_slice(&24u32.to_le_bytes());
        odd.extend(pcb_group(peer, ours, 2, 501));
        assert_eq!(match_pcb(&parse_pcblist_n(&odd), ours, peer), None, "a socket record that is not a TCP stream");
    }

    /// Only an ESTABLISHED row is read — a `TIME_WAIT` row of an earlier connection on the same addresses shows
    /// uid 0 (#644's round-4 review, finding 3; seen failing first with any state: 0 was returned).
    #[test]
    fn a_proc_net_tcp_row_is_matched_on_both_addresses_when_established() {
        let local: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let (l, p) = (proc_spelling(local), proc_spelling(peer));
        let text = format!(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: {l} 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1\n\
             1: {p} {l} 06 00000000:00000000 00:00000000 00000000     0        0 0\n\
             2: {p} {l} 01 00000000:00000000 00:00000000 00000000  1001        0 2\n\
             3: {l} {p} 01 00000000:00000000 00:00000000 00000000  1000        0 3\n"
        );
        assert_eq!(parse_proc_net_tcp(&text, local, peer), Some(1001), "the client's established row, not TIME_WAIT's, the listener's or ours");
        assert_eq!(parse_proc_net_tcp(&text, local, "127.0.0.1:40001".parse().unwrap()), None);
    }

    /// #644's round-5 review: a client that half-closed is still named (FIN_WAIT1/2, CLOSE_WAIT), `TIME_WAIT` never
    /// (finding 4; seen failing first with ESTABLISHED only: the half-closed client was refused); and a dual-stack
    /// client of the IPv4 listener is found under its IPv4-mapped spelling in `tcp6` (finding 1; seen failing
    /// first: an IPv4 peer was looked up in `tcp` alone).
    #[test]
    fn half_closed_and_dual_stack_clients_are_named() {
        let local: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let (l, p) = (proc_spelling(local), proc_spelling(peer));
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        for state in ["01", "04", "05", "08"] {
            let t = format!("{header}0: {p} {l} {state} 00000000:00000000 00:00000000 00000000  1001        0 2\n");
            assert_eq!(parse_proc_net_tcp(&t, local, peer), Some(1001), "state {state}");
        }
        let t = format!("{header}0: {p} {l} 06 00000000:00000000 00:00000000 00000000     0        0 0\n");
        assert_eq!(parse_proc_net_tcp(&t, local, peer), None, "TIME_WAIT");
        // The dual-stack client: only in tcp6, as ::ffff:127.0.0.1.
        let (ml, mp) = ("0000000000000000FFFF00000100007F:1F90", "0000000000000000FFFF00000100007F:9C40");
        assert_eq!(proc_spelling(SocketAddr::new("::ffff:127.0.0.1".parse().unwrap(), 8080)), ml);
        let tcp6 = format!("{header}0: {mp} {ml} 01 00000000:00000000 00:00000000 00000000  1002        0 3\n");
        assert_eq!(lookup_in_tables(Some(header), Some(&tcp6), local, peer), Some(1002));
        assert_eq!(lookup_in_tables(Some(header), None, local, peer), None);
        // macOS: a mapped inpcb row matches the IPv4 peer.
        let rows = vec![("::ffff:127.0.0.1".parse().unwrap(), 40000, "::ffff:127.0.0.1".parse().unwrap(), 8080, 1003)];
        assert_eq!(match_pcb(&rows, local, peer), Some(1003));
    }

    /// A child that does not finish is killed at its bound (the macOS `sysctl` read, round 5).
    #[test]
    fn a_command_past_its_bound_is_killed() {
        let started = std::time::Instant::now();
        let e = run_bounded("/bin/sleep", &["5"], 200).unwrap_err();
        assert!(e.contains("killed") && started.elapsed() < std::time::Duration::from_secs(2), "{e} {:?}", started.elapsed());
        assert_eq!(run_bounded("/bin/echo", &["hi"], 2_000).unwrap(), b"hi\n");
    }

    /// A dual-stack client — a socket of the IPv6 family reaching the IPv4 listener at `::ffff:127.0.0.1` — is named
    /// by the kernel's own table, live (where the platform lets an IPv6 socket reach a mapped address).
    #[test]
    fn a_live_dual_stack_client_is_named() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local = listener.local_addr().unwrap();
        let Ok(client) = std::net::TcpStream::connect(SocketAddr::new("::ffff:127.0.0.1".parse().unwrap(), local.port())) else {
            eprintln!("a_live_dual_stack_client_is_named: SKIPPED — this host does not connect an IPv6 socket to a mapped address");
            return;
        };
        let (_server, peer) = listener.accept().unwrap();
        assert!(client.local_addr().unwrap().is_ipv6(), "the client is of the IPv6 family");
        assert_eq!(loopback_peer_uid(local, peer), Ok(own_uid()));
    }

    /// #644's round-7 review: the `sock_diag` request for one socket, byte for byte (the netlink header in native
    /// order, ports and addresses in network order), and its answer parsed — the uid and state, the kernel's
    /// `ENOENT`, an answer to another request, an answer about another socket.
    #[test]
    fn a_sock_diag_request_and_its_answer_are_the_kernels_bytes() {
        let client: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let ours: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        // nlmsghdr: len 72, SOCK_DIAG_BY_FAMILY, NLM_F_REQUEST, seq 7, pid 0.
        let header = |len: u32, kind: u16, flags: u16, seq: u32| [&len.to_ne_bytes()[..], &kind.to_ne_bytes(), &flags.to_ne_bytes(), &seq.to_ne_bytes(), &[0; 4]].concat();
        let mut want = header(72, 20, 1, 7);
        want.extend_from_slice(&[2, 6, 0, 0, 0xff, 0xff, 0xff, 0xff]); // AF_INET, IPPROTO_TCP, ext, pad, every state
        want.extend_from_slice(&[0x9c, 0x40, 0x1f, 0x90]); // sport 40000, dport 8080, network order
        want.extend_from_slice(&[127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        want.extend_from_slice(&[127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        want.extend_from_slice(&[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // if, INET_DIAG_NOCOOKIE
        assert_eq!(diag::request(7, client, ours), want);
        // A dual-stack client is asked for by its IPv4 address.
        let mapped = SocketAddr::new("::ffff:127.0.0.1".parse().unwrap(), 40000);
        assert_eq!(diag::request(7, mapped, ours), want);
        // An IPv6 pair: AF_INET6, sixteen-byte addresses.
        let v6 = diag::request(1, "[::1]:40000".parse().unwrap(), "[::1]:8080".parse().unwrap());
        assert_eq!((v6[16], &v6[24..28], v6[43], v6[59]), (10, &[0x9c, 0x40, 0x1f, 0x90][..], 1, 1));
        // The answer: nlmsghdr, then inet_diag_msg — family, state, timer, retrans, the sockid, expires, rqueue,
        // wqueue, uid, inode.
        let answer = |seq: u32, state: u8, sport: u16| {
            let mut a = header(88, 20, 0, seq);
            a.extend_from_slice(&[2, state, 0, 0]);
            a.extend_from_slice(&sport.to_be_bytes());
            a.extend_from_slice(&[0x1f, 0x90]);
            a.extend_from_slice(&[127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            a.extend_from_slice(&[127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            a.extend_from_slice(&[0, 0, 0, 0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x08]); // if, cookie
            a.extend_from_slice(&[0; 12]); // expires, rqueue, wqueue
            a.extend_from_slice(&1001u32.to_ne_bytes());
            a.extend_from_slice(&5u32.to_ne_bytes());
            a
        };
        assert_eq!(answer(7, 1, 40000).len(), 88);
        assert_eq!(diag::parse(&answer(7, 1, 40000), 7, client, ours), Ok((1, 1001)));
        assert_eq!(diag::parse(&answer(7, 5, 40000), 7, client, ours), Ok((5, 1001)));
        assert!(diag::parse(&answer(6, 1, 40000), 7, client, ours).unwrap_err().contains("another request"));
        assert!(diag::parse(&answer(7, 1, 40001), 7, client, ours).unwrap_err().contains("another socket"));
        // A dual-stack socket, which the kernel describes as AF_INET6 with IPv4-mapped addresses.
        let mut v6answer = header(88, 20, 0, 7);
        v6answer.extend_from_slice(&[10, 1, 0, 0, 0x9c, 0x40, 0x1f, 0x90]);
        for _ in 0..2 {
            v6answer.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 127, 0, 0, 1]);
        }
        v6answer.extend_from_slice(&[0; 24]);
        v6answer.extend_from_slice(&1002u32.to_ne_bytes());
        v6answer.extend_from_slice(&6u32.to_ne_bytes());
        assert_eq!(v6answer.len(), 88);
        assert_eq!(diag::parse(&v6answer, 7, client, ours), Ok((1, 1002)));
        let mut enoent = header(36, 2, 0, 7);
        enoent.extend_from_slice(&(-2i32).to_ne_bytes());
        enoent.extend_from_slice(&diag::request(7, client, ours)[..16]);
        assert!(diag::parse(&enoent, 7, client, ours).unwrap_err().contains("ENOENT"));
        assert!(diag::parse(&[1, 2, 3], 7, client, ours).is_err());
    }

    /// Live, Linux: the kernel's `sock_diag` answer names this user's own connection, a dual-stack client, and a
    /// client that has half-closed (FIN_WAIT2); a client in TIME_WAIT is not named.
    #[cfg(target_os = "linux")]
    #[test]
    fn sock_diag_names_own_dual_stack_and_half_closed_clients_and_not_time_wait() {
        let me = own_uid();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local = listener.local_addr().unwrap();
        let plain = std::net::TcpStream::connect(local).unwrap();
        let (_s1, p1) = listener.accept().unwrap();
        let dual = std::net::TcpStream::connect(SocketAddr::new("::ffff:127.0.0.1".parse().unwrap(), local.port())).expect("dual-stack connect");
        assert!(dual.local_addr().unwrap().is_ipv6());
        let (_s2, p2) = listener.accept().unwrap();
        let half = std::net::TcpStream::connect(local).unwrap();
        let (_s3, p3) = listener.accept().unwrap();
        half.shutdown(std::net::Shutdown::Write).unwrap();
        let gone = std::net::TcpStream::connect(local).unwrap();
        let (s4, p4) = listener.accept().unwrap();
        drop(gone); // the client closes first …
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(s4); // … then the server: the client's socket is in TIME_WAIT
        std::thread::sleep(std::time::Duration::from_millis(200));
        let answers = os::diag_only(&[(local, p1), (local, p2), (local, p3), (local, p4)]).expect("a sock_diag socket opens");
        assert_eq!(answers[0], Ok(me), "own");
        assert_eq!(answers[1], Ok(me), "dual-stack");
        assert_eq!(answers[2], Ok(me), "half-closed");
        assert!(answers[3].as_ref().unwrap_err().contains("not a live connection") || answers[3].as_ref().unwrap_err().contains("ENOENT"), "TIME_WAIT: {:?}", answers[3]);
        // And the whole path the proxy calls agrees.
        assert_eq!(loopback_peer_uid(local, p2), Ok(me));
        drop((plain, dual, half));
    }

    /// One read answers many: a batch of pairs comes back in order, each its own answer.
    #[test]
    fn one_read_answers_a_batch_in_order() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local = listener.local_addr().unwrap();
        let clients: Vec<_> = (0..3).map(|_| std::net::TcpStream::connect(local).unwrap()).collect();
        // The accepted ends are kept: dropping one closes the connection, and its client's row leaves ESTABLISHED.
        let accepted: Vec<(std::net::TcpStream, SocketAddr)> = (0..3).map(|_| listener.accept().unwrap()).collect();
        let peers: Vec<SocketAddr> = accepted.iter().map(|(_, p)| *p).collect();
        let gone: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut pairs: Vec<_> = peers.iter().map(|p| (local, *p)).collect();
        pairs.insert(1, (local, gone));
        let got = loopback_peer_uids(&pairs);
        assert_eq!(got.len(), 4);
        assert!(got[1].is_err(), "{got:?}");
        assert!([0, 2, 3].iter().all(|i| got[*i] == Ok(own_uid())), "{got:?}");
        drop((clients, accepted));
    }
}
