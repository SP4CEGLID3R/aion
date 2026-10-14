//! Link layer → IP → TCP parsing for captured packets.
//!
//! Only what the radar needs: TCP segments over IPv4/IPv6 on the link types
//! Npcap and libpcap commonly hand out. Fragments, non-TCP traffic and
//! malformed packets are skipped (`None`), never guessed at.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// pcap link-layer header types (<https://www.tcpdump.org/linktypes.html>).
pub mod linktype {
    pub const NULL: u32 = 0;
    pub const ETHERNET: u32 = 1;
    pub const RAW: u32 = 101;
    pub const LOOP: u32 = 108;
    pub const LINUX_SLL: u32 = 113;
    pub const IPV4: u32 = 228;
    pub const IPV6: u32 = 229;
    pub const LINUX_SLL2: u32 = 276;
}

pub mod tcp_flags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpSegment<'a> {
    pub src: SocketAddr,
    pub dst: SocketAddr,
    pub seq: u32,
    pub flags: u8,
    pub payload: &'a [u8],
}

impl TcpSegment<'_> {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// Where the IP packet starts inside a captured frame, per link type.
pub fn ip_offset(linktype: u32, frame: &[u8]) -> Option<usize> {
    match linktype {
        linktype::RAW | linktype::IPV4 | linktype::IPV6 => Some(0),
        // 4-byte address family; any value is fine, the IP version nibble decides.
        linktype::NULL | linktype::LOOP => (frame.len() >= 4).then_some(4),
        linktype::ETHERNET => {
            let mut off = 12;
            loop {
                let ethertype = u16::from_be_bytes([*frame.get(off)?, *frame.get(off + 1)?]);
                match ethertype {
                    // 802.1Q / 802.1ad VLAN tags.
                    0x8100 | 0x88A8 => off += 4,
                    0x0800 | 0x86DD => return Some(off + 2),
                    _ => return None,
                }
            }
        }
        linktype::LINUX_SLL => (frame.len() >= 16).then_some(16),
        linktype::LINUX_SLL2 => (frame.len() >= 20).then_some(20),
        _ => None,
    }
}

/// Parses a captured frame into a TCP segment.
pub fn parse_frame(linktype: u32, frame: &[u8]) -> Option<TcpSegment<'_>> {
    parse_ip(&frame[ip_offset(linktype, frame)?..])
}

/// The IP packet within `frame` (trimmed to its declared length), for
/// recording as raw IP.
pub fn ip_packet(linktype: u32, frame: &[u8]) -> Option<&[u8]> {
    let ip = frame.get(ip_offset(linktype, frame)?..)?;
    let len = match ip.first()? >> 4 {
        4 => u16::from_be_bytes([*ip.get(2)?, *ip.get(3)?]) as usize,
        6 => 40 + u16::from_be_bytes([*ip.get(4)?, *ip.get(5)?]) as usize,
        _ => return None,
    };
    ip.get(..len.min(ip.len()))
}

pub fn parse_ip(ip: &[u8]) -> Option<TcpSegment<'_>> {
    match ip.first()? >> 4 {
        4 => parse_ipv4(ip),
        6 => parse_ipv6(ip),
        _ => None,
    }
}

fn parse_ipv4(ip: &[u8]) -> Option<TcpSegment<'_>> {
    let ihl = ((ip.first()? & 0x0F) as usize) * 4;
    if ihl < 20 || ip.len() < ihl {
        return None;
    }
    let total = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    // Ethernet pads short frames; the IP length is authoritative. TSO captures
    // can report 0, in which case the captured length is used.
    let end = if total == 0 {
        ip.len()
    } else {
        total.min(ip.len())
    };
    if end < ihl {
        return None;
    }
    let frag = u16::from_be_bytes([ip[6], ip[7]]);
    let more_fragments = frag & 0x2000 != 0;
    let offset = frag & 0x1FFF;
    if more_fragments || offset != 0 || ip[9] != 6 {
        return None;
    }
    let src = IpAddr::V4(Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]));
    let dst = IpAddr::V4(Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]));
    parse_tcp(src, dst, &ip[ihl..end])
}

fn parse_ipv6(ip: &[u8]) -> Option<TcpSegment<'_>> {
    if ip.len() < 40 {
        return None;
    }
    let payload_len = u16::from_be_bytes([ip[4], ip[5]]) as usize;
    let end = (40 + payload_len).min(ip.len());
    let src: [u8; 16] = ip[8..24].try_into().ok()?;
    let dst: [u8; 16] = ip[24..40].try_into().ok()?;
    let mut next = ip[6];
    let mut off = 40;
    // Skip hop-by-hop, routing and destination-options headers.
    while matches!(next, 0 | 43 | 60) {
        let hdr = ip.get(off..off + 2)?;
        next = hdr[0];
        off += (hdr[1] as usize + 1) * 8;
    }
    if next != 6 || off > end {
        return None;
    }
    parse_tcp(
        IpAddr::V6(Ipv6Addr::from(src)),
        IpAddr::V6(Ipv6Addr::from(dst)),
        &ip[off..end],
    )
}

fn parse_tcp(src: IpAddr, dst: IpAddr, tcp: &[u8]) -> Option<TcpSegment<'_>> {
    if tcp.len() < 20 {
        return None;
    }
    let data_off = ((tcp[12] >> 4) as usize) * 4;
    if data_off < 20 || tcp.len() < data_off {
        return None;
    }
    Some(TcpSegment {
        src: SocketAddr::new(src, u16::from_be_bytes([tcp[0], tcp[1]])),
        dst: SocketAddr::new(dst, u16::from_be_bytes([tcp[2], tcp[3]])),
        seq: u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]),
        flags: tcp[13],
        payload: &tcp[data_off..],
    })
}

/// Builds an IPv4/TCP packet. For tests and synthetic fixtures.
pub fn build_ipv4_tcp(
    src: SocketAddr,
    dst: SocketAddr,
    seq: u32,
    flags: u8,
    payload: &[u8],
) -> Vec<u8> {
    let (IpAddr::V4(s), IpAddr::V4(d)) = (src.ip(), dst.ip()) else {
        panic!("build_ipv4_tcp needs IPv4 addresses");
    };
    let total = 20 + 20 + payload.len();
    let mut p = Vec::with_capacity(total);
    p.extend_from_slice(&[
        0x45,
        0,
        (total >> 8) as u8,
        total as u8,
        0,
        0,
        0x40,
        0,
        64,
        6,
        0,
        0,
    ]);
    p.extend_from_slice(&s.octets());
    p.extend_from_slice(&d.octets());
    p.extend_from_slice(&src.port().to_be_bytes());
    p.extend_from_slice(&dst.port().to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0, 0x50, flags | 0x10, 0xFF, 0xFF, 0, 0, 0, 0]);
    p.extend_from_slice(payload);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn ethernet(ip: &[u8], vlan: bool) -> Vec<u8> {
        let mut f = vec![0u8; 12];
        if vlan {
            f.extend_from_slice(&[0x81, 0x00, 0x00, 0x05]);
        }
        f.extend_from_slice(&[0x08, 0x00]);
        f.extend_from_slice(ip);
        f.extend_from_slice(&[0u8; 6]); // Ethernet padding past the IP length
        f
    }

    #[test]
    fn parses_ipv4_tcp_on_every_link_type() {
        let ip = build_ipv4_tcp(
            addr("10.0.0.2:5000"),
            addr("1.2.3.4:443"),
            77,
            tcp_flags::SYN,
            b"hello",
        );
        let mut null = vec![2, 0, 0, 0];
        null.extend_from_slice(&ip);
        let mut sll = vec![0u8; 16];
        sll.extend_from_slice(&ip);
        for (lt, frame) in [
            (linktype::RAW, ip.clone()),
            (linktype::ETHERNET, ethernet(&ip, false)),
            (linktype::ETHERNET, ethernet(&ip, true)),
            (linktype::NULL, null),
            (linktype::LINUX_SLL, sll),
        ] {
            let seg = parse_frame(lt, &frame).unwrap_or_else(|| panic!("linktype {lt}"));
            assert_eq!(seg.src, addr("10.0.0.2:5000"));
            assert_eq!(seg.dst, addr("1.2.3.4:443"));
            assert_eq!(seg.seq, 77);
            assert!(seg.has(tcp_flags::SYN));
            assert_eq!(
                seg.payload, b"hello",
                "linktype {lt}: padding must be trimmed"
            );
        }
    }

    #[test]
    fn ip_packet_is_trimmed_to_its_length() {
        let ip = build_ipv4_tcp(addr("10.0.0.2:5000"), addr("1.2.3.4:443"), 1, 0, b"x");
        assert_eq!(
            ip_packet(linktype::ETHERNET, &ethernet(&ip, false)).unwrap(),
            &ip[..]
        );
    }

    #[test]
    fn skips_non_tcp_fragments_and_garbage() {
        let mut udp = build_ipv4_tcp(addr("10.0.0.2:5000"), addr("1.2.3.4:443"), 1, 0, b"x");
        udp[9] = 17;
        assert!(parse_ip(&udp).is_none());

        let mut frag = build_ipv4_tcp(addr("10.0.0.2:5000"), addr("1.2.3.4:443"), 1, 0, b"x");
        frag[6] = 0x20; // more fragments
        assert!(parse_ip(&frag).is_none());

        assert!(parse_ip(&[]).is_none());
        assert!(parse_ip(&[0x45, 0, 0]).is_none());
        assert!(parse_frame(linktype::ETHERNET, &[0u8; 10]).is_none());
        assert!(parse_frame(9999, &[0x45; 60]).is_none());
    }

    #[test]
    fn parses_ipv6_tcp() {
        let mut ip = vec![0x60, 0, 0, 0, 0, 23, 6, 64];
        ip.extend_from_slice(&[0u8; 15]);
        ip.push(1); // ::1
        ip.extend_from_slice(&[0u8; 15]);
        ip.push(1);
        let mut tcp = vec![
            0x13, 0x88, 0x01, 0xBB, 0, 0, 0, 9, 0, 0, 0, 0, 0x50, 0x18, 0, 0, 0, 0, 0, 0,
        ];
        tcp.extend_from_slice(b"abc");
        ip.extend_from_slice(&tcp);
        let seg = parse_ip(&ip).unwrap();
        assert_eq!(seg.src, addr("[::1]:5000"));
        assert_eq!(seg.dst.port(), 443);
        assert_eq!(seg.payload, b"abc");
    }
}
