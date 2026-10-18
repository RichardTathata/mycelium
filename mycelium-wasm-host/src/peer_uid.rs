//! **Who is on the other end of a loopback connection** (#644's round-3 review): the user id of the process
//! that owns the client socket of a TCP connection to this host, read from the kernel, so an activation's egress
//! proxy can refuse at accept every connection that is not the stem's own user's — another user on the host
//! cannot hold a place in the proxy's accounting, token or not.
//!
//! - **Linux:** `/proc/net/tcp` (and `tcp6`), the ESTABLISHED row whose local address is the peer's and whose
//!   remote address is ours; its `uid` column.
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

/// The uid on the **ESTABLISHED** (`01`) `/proc/net/tcp{,6}` row whose local address is `peer` and remote address
/// `local` — a `TIME_WAIT` row of an earlier connection on the same addresses shows uid 0 and is never read
/// (#644's round-4 review, finding 3).
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_proc_net_tcp(text: &str, local: SocketAddr, peer: SocketAddr) -> Option<u32> {
    let (want_local, want_remote) = (proc_spelling(peer), proc_spelling(local));
    text.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        (f.len() > 7 && f[3] == "01" && f[1].eq_ignore_ascii_case(&want_local) && f[2].eq_ignore_ascii_case(&want_remote))
            .then(|| f[7].parse::<u32>().ok())
            .flatten()
    })
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
    rows.iter()
        .find(|(la, lp, fa, fp, _)| *la == peer.ip() && *lp == peer.port() && *fa == local.ip() && *fp == local.port())
        .map(|r| r.4)
}

#[cfg(target_os = "linux")]
mod os {
    use std::net::SocketAddr;

    pub(super) fn lookup_all(pairs: &[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> {
        let read = |table: &str| std::fs::read_to_string(table).map_err(|e| format!("{table}: {e}"));
        let v4 = pairs.iter().any(|(_, p)| p.is_ipv4()).then(|| read("/proc/net/tcp"));
        let v6 = pairs.iter().any(|(_, p)| p.is_ipv6()).then(|| read("/proc/net/tcp6"));
        pairs
            .iter()
            .map(|(local, peer)| {
                let text = if peer.is_ipv4() { v4.as_ref() } else { v6.as_ref() };
                match text {
                    Some(Ok(t)) => super::parse_proc_net_tcp(t, *local, *peer).ok_or_else(|| format!("{peer} → {local} is not an established connection")),
                    Some(Err(e)) => Err(e.clone()),
                    None => Err("no table read".into()),
                }
            })
            .collect()
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::net::SocketAddr;

    pub(super) fn lookup_all(pairs: &[(SocketAddr, SocketAddr)]) -> Vec<Result<u32, String>> {
        let rows = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-b", "net.inet.tcp.pcblist_n"])
            .output()
            .map_err(|e| format!("sysctl net.inet.tcp.pcblist_n: {e}"))
            .and_then(|out| {
                if out.status.success() {
                    Ok(super::parse_pcblist_n(&out.stdout))
                } else {
                    Err(format!("sysctl net.inet.tcp.pcblist_n exited {}", out.status))
                }
            });
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
