//! **Who is on the other end of a loopback connection** (#644's round-3 review): the user id of the process
//! that owns the client socket of a TCP connection to this host, read from the kernel, so an activation's egress
//! proxy can refuse at accept every connection that is not the stem's own user's — another user on the host
//! cannot hold a place in the proxy's accounting, token or not.
//!
//! - **Linux:** `/proc/net/tcp` (and `tcp6`), the row whose local address is the peer's and whose remote
//!   address is ours; its `uid` column.
//! - **macOS:** `sysctl -b net.inet.tcp.pcblist_n` (run as `/usr/sbin/sysctl`, so no `unsafe` call): the
//!   `XSO_INPCB` record whose ports are the peer's and ours, then the `XSO_SOCKET` record that follows it in
//!   the same group; its `so_uid` (offset 64 of `struct xsocket_n`, `#pragma pack(4)` in xnu's
//!   `bsd/netinet/in_pcb.h` — confirmed by `the_kernel_names_this_users_own_loopback_connection`).
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
    os::lookup(local, peer)
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

/// The uid on the `/proc/net/tcp{,6}` row whose local address is `peer` and remote address `local`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_proc_net_tcp(text: &str, local: SocketAddr, peer: SocketAddr) -> Option<u32> {
    let (want_local, want_remote) = (proc_spelling(peer), proc_spelling(local));
    text.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        (f.len() > 7 && f[1].eq_ignore_ascii_case(&want_local) && f[2].eq_ignore_ascii_case(&want_remote))
            .then(|| f[7].parse::<u32>().ok())
            .flatten()
    })
}

/// The `so_uid` of the socket whose `XSO_INPCB` record has local port `peer_port` and foreign port `our_port`, in
/// a `net.inet.tcp.pcblist_n` buffer.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn parse_pcblist_n(data: &[u8], our_port: u16, peer_port: u16) -> Option<u32> {
    const XSO_SOCKET: u32 = 0x001;
    const XSO_INPCB: u32 = 0x010;
    const SO_UID: usize = 64;
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let be16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let mut off = u32_at(0)? as usize; // the leading `struct xinpgen`
    let mut matched = false;
    while off + 8 <= data.len() {
        let (len, kind) = (u32_at(off)? as usize, u32_at(off + 4)?);
        if len < 8 {
            break;
        }
        if kind == XSO_INPCB {
            // `inp_fport` at 16, `inp_lport` at 18, network order.
            matched = be16_at(off + 16)? == our_port && be16_at(off + 18)? == peer_port;
        } else if kind == XSO_SOCKET && matched {
            return if len > SO_UID + 4 { u32_at(off + SO_UID) } else { None };
        }
        off += (len + 7) & !7;
    }
    None
}

#[cfg(target_os = "linux")]
mod os {
    use std::net::SocketAddr;

    pub(super) fn lookup(local: SocketAddr, peer: SocketAddr) -> Result<u32, String> {
        let table = if peer.is_ipv4() { "/proc/net/tcp" } else { "/proc/net/tcp6" };
        let text = std::fs::read_to_string(table).map_err(|e| format!("{table}: {e}"))?;
        super::parse_proc_net_tcp(&text, local, peer).ok_or_else(|| format!("{peer} → {local} is not in {table}"))
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::net::SocketAddr;

    pub(super) fn lookup(local: SocketAddr, peer: SocketAddr) -> Result<u32, String> {
        let out = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-b", "net.inet.tcp.pcblist_n"])
            .output()
            .map_err(|e| format!("sysctl net.inet.tcp.pcblist_n: {e}"))?;
        if !out.status.success() {
            return Err(format!("sysctl net.inet.tcp.pcblist_n exited {}", out.status));
        }
        super::parse_pcblist_n(&out.stdout, local.port(), peer.port()).ok_or_else(|| format!("{peer} → {local} is not in net.inet.tcp.pcblist_n"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod os {
    use std::net::SocketAddr;

    pub(super) fn lookup(_local: SocketAddr, _peer: SocketAddr) -> Result<u32, String> {
        Err(format!("no loopback peer-uid lookup on {}", std::env::consts::OS))
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

    #[test]
    fn a_proc_net_tcp_row_is_matched_on_both_addresses() {
        let local: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let (l, p) = (proc_spelling(local), proc_spelling(peer));
        let text = format!(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: {l} 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1\n\
             1: {p} {l} 01 00000000:00000000 00:00000000 00000000  1001        0 2\n\
             2: {l} {p} 01 00000000:00000000 00:00000000 00000000  1000        0 3\n"
        );
        assert_eq!(parse_proc_net_tcp(&text, local, peer), Some(1001), "the client's row, not the listener's or ours");
        assert_eq!(parse_proc_net_tcp(&text, local, "127.0.0.1:40001".parse().unwrap()), None);
    }
}
