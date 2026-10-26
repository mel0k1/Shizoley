use x86_64::PhysAddr;

// host-side rx smoke: the same descriptor math and packet layout the
// driver-vm guest uses, driven from the kernel right after the dma/intx
// smoke (which leaves the device released). the round trips against qemu's
// user-net stack:
//   arp request (who has 10.0.2.2) -> arp reply -> icmp echo -> echo reply;
//   dhcp discover as udp 68 -> 67 broadcast -> offer (slirp's built-in
//   server, no external resolver involved) — the udp half of the stack,
//   proven end to end without any host-side sender.
// ims is armed so the replies ride the nic's intx line (the same pic
// vector the dma smoke discovered), and icr is written back after every
// drain — a level line nobody clears re-fires after each eoi. under tcg
// this is the only full network proof that can run at all; the guest half
// of the same path runs in devd on a vmx host.

use crate::dma::{enable_device, find_device, mmio_bar};

const ICR: usize = 0x00C0;
const IMS: usize = 0x00D0;
const IMC: usize = 0x00D8;
const RCTL: usize = 0x0100;
const CTRL: usize = 0x0000;
const TCTL: usize = 0x0400;
const RDBAL: usize = 0x2800;
const RDBAH: usize = 0x2804;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const TDBAL: usize = 0x3800;
const TDBAH: usize = 0x3804;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const RAL: usize = 0x5400;
const RAH: usize = 0x5404;

const NDESC: usize = 8;
// dma view inside the smoke's iommu domain: rx ring at 0x4000, tx ring at
// 0x4100 (one frame holds both), rx buffers at 0x8000, tx buffers at 0xa000
const RXRING_GPA: u64 = 0x4000;
const TXRING_GPA: u64 = 0x4100;
const RXBUF_GPA: u64 = 0x8000;
const TXBUF_GPA: u64 = 0xA000;
const RXBUF_SIZE: usize = 2048;

// the slirp corner of qemu user-net (10.0.2.99 as our own address: outside
// the set slirp itself answers for, so the arp sender is never mistaken
// for one of the backend's identities)
const OUR_IP: [u8; 4] = [10, 0, 2, 99];
const GW_IP: [u8; 4] = [10, 0, 2, 2];
const DHCP_XID: [u8; 4] = [0x53, 0x48, 0x5A, 0x01]; // "shz" + 1

struct Regs(usize);

impl Regs {
    fn rd(&self, r: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.0 + r) as *const u32) }
    }
    fn wr(&self, r: usize, v: u32) {
        unsafe { core::ptr::write_volatile((self.0 + r) as *mut u32, v) }
    }
}

fn mac_str(mac: &[u8; 6], out: &mut [u8; 17]) {
    let hex = b"0123456789abcdef";
    for i in 0..6 {
        out[i * 3] = hex[(mac[i] >> 4) as usize];
        out[i * 3 + 1] = hex[(mac[i] & 0xF) as usize];
        if i != 5 {
            out[i * 3 + 2] = b':';
        }
    }
}

fn mac_view<'a>(mac: &[u8; 6], out: &'a mut [u8; 17]) -> &'a str {
    mac_str(mac, out);
    core::str::from_utf8(out).unwrap_or("?")
}

// ip checksum over big-endian words
fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += ((data[i] as u32) << 8) | data[i + 1] as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn poke(frame: PhysAddr, off: usize, data: &[u8]) {
    unsafe {
        let p = crate::memory::phys_to_virt(frame)
            .as_mut_ptr::<u8>()
            .add(off);
        for (i, b) in data.iter().enumerate() {
            core::ptr::write_volatile(p.add(i), *b);
        }
    }
}

fn peek(frame: PhysAddr, off: usize, data: &mut [u8]) {
    unsafe {
        let p = crate::memory::phys_to_virt(frame)
            .as_mut_ptr::<u8>()
            .add(off);
        for (i, b) in data.iter_mut().enumerate() {
            *b = core::ptr::read_volatile(p.add(i));
        }
    }
}

// legacy rx descriptor: addr(8) len(2) csum(2) status(1) errors(1) special(2)
fn rx_status(ring: PhysAddr, i: usize) -> u8 {
    unsafe {
        core::ptr::read_volatile(
            crate::memory::phys_to_virt(ring)
                .as_mut_ptr::<u8>()
                .add(i * 16 + 12),
        )
    }
}

fn rx_clear(ring: PhysAddr, i: usize) {
    unsafe {
        core::ptr::write_volatile(
            crate::memory::phys_to_virt(ring)
                .as_mut_ptr::<u8>()
                .add(i * 16 + 12),
            0,
        );
    }
}

fn rx_len(ring: PhysAddr, i: usize) -> u16 {
    unsafe {
        core::ptr::read_volatile(
            crate::memory::phys_to_virt(ring)
                .as_mut_ptr::<u16>()
                .add(i * 8 + 4),
        )
    }
}

// legacy tx descriptor: addr(8) len(2) cso(1) cmd(1) status(1) ...
fn tx_post(ring: PhysAddr, slot: usize, gpa: u64, len: usize) {
    unsafe {
        let p = crate::memory::phys_to_virt(ring)
            .as_mut_ptr::<u8>()
            .add(slot * 16);
        core::ptr::write_volatile(p as *mut u64, gpa);
        core::ptr::write_volatile(p.add(8) as *mut u16, len as u16);
        core::ptr::write_volatile(p.add(11), 0x0B); // eop | ifcs | rs
        core::ptr::write_volatile(p.add(12), 0);
    }
}

fn tx_slot_done(ring: PhysAddr, slot: usize) -> bool {
    unsafe {
        core::ptr::read_volatile(
            crate::memory::phys_to_virt(ring)
                .as_mut_ptr::<u8>()
                .add(slot * 16 + 12),
        ) & 1
            != 0
    }
}

// bounded wait for the tx writeback: a couple of scheduler ticks, hlt
// between checks so the rest of the machine keeps running
fn tx_done(ring: PhysAddr, slot: usize) -> bool {
    if tx_slot_done(ring, slot) {
        return true;
    }
    let start = crate::interrupts::tick_count();
    while crate::interrupts::tick_count() < start + 2 {
        x86_64::instructions::interrupts::enable_and_hlt();
        if tx_slot_done(ring, slot) {
            return true;
        }
    }
    tx_slot_done(ring, slot)
}

// the rx cursor: hw fills up to rdt (inclusive), software consumes from
// (rdt + 1) — the one-descriptor slack the 8254x scheme wants
struct Ring {
    cursor: usize,
}

impl Ring {
    fn new(regs: &Regs) -> Ring {
        Ring {
            cursor: (regs.rd(RDT) as usize + 1) % NDESC,
        }
    }
}

fn free_frames(
    ring: x86_64::structures::paging::PhysFrame,
    bufs: &[x86_64::structures::paging::PhysFrame],
) {
    crate::memory::dealloc_frame(ring);
    for f in bufs {
        crate::memory::dealloc_frame(*f);
    }
}

fn tx_arp_request(regs: &Regs, txring: PhysAddr, txbuf: PhysAddr, slot: usize, mac: &[u8; 6]) {
    let mut f = [0u8; 42];
    f[0..6].copy_from_slice(&[0xFF; 6]);
    f[6..12].copy_from_slice(mac);
    f[12..14].copy_from_slice(&[0x08, 0x06]);
    f[14..16].copy_from_slice(&[0x00, 0x01]);
    f[16..18].copy_from_slice(&[0x08, 0x00]);
    f[18] = 6;
    f[19] = 4;
    f[20..22].copy_from_slice(&[0x00, 0x01]); // request
    f[22..28].copy_from_slice(mac);
    f[28..32].copy_from_slice(&OUR_IP);
    f[38..42].copy_from_slice(&GW_IP);
    let gpa = TXBUF_GPA + (slot * RXBUF_SIZE) as u64;
    poke(txbuf, slot * RXBUF_SIZE, &f);
    tx_post(txring, slot, gpa, f.len());
    regs.wr(TDT, (slot + 1) as u32);
}

fn tx_icmp_echo(
    regs: &Regs,
    txring: PhysAddr,
    txbuf: PhysAddr,
    slot: usize,
    mac: &[u8; 6],
    gw_mac: &[u8; 6],
) {
    let mut f = [0u8; 63];
    let ip = 14usize;
    let ic = ip + 20usize;
    f[0..6].copy_from_slice(gw_mac);
    f[6..12].copy_from_slice(mac);
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    f[ip] = 0x45;
    f[ip + 2..ip + 4].copy_from_slice(&49u16.to_be_bytes());
    f[ip + 4..ip + 6].copy_from_slice(&2u16.to_be_bytes()); // id
    f[ip + 8] = 64; // ttl
    f[ip + 9] = 1; // icmp
    f[ip + 12..ip + 16].copy_from_slice(&OUR_IP);
    f[ip + 16..ip + 20].copy_from_slice(&GW_IP);
    f[ic] = 8; // echo request
    f[ic + 4] = 0;
    f[ic + 5] = 2; // seq
    let payload = b"paparaci rx smoke";
    f[ic + 8..ic + 8 + payload.len()].copy_from_slice(payload);
    let c = checksum(&f[ip..ip + 20]);
    f[ip + 10..ip + 12].copy_from_slice(&c.to_be_bytes());
    let c2 = checksum(&f[ic..ic + 29]);
    f[ic + 2..ic + 4].copy_from_slice(&c2.to_be_bytes());
    let gpa = TXBUF_GPA + (slot * RXBUF_SIZE) as u64;
    poke(txbuf, slot * RXBUF_SIZE, &f);
    tx_post(txring, slot, gpa, 63);
    regs.wr(TDT, (slot + 1) as u32);
}

// dhcp discover as a udp datagram: 68 -> 67 broadcast, src 0.0.0.0. the
// udp checksum is left zero — ipv4 lets a sender skip it, slirp honors
// that, and the reply is the offer from qemu's built-in server
fn tx_dhcp_discover(regs: &Regs, txring: PhysAddr, txbuf: PhysAddr, slot: usize, mac: &[u8; 6]) {
    let mut f = [0u8; 286];
    let ip = 14usize;
    let up = ip + 20usize;
    let b = up + 8usize;
    f[0..6].copy_from_slice(&[0xFF; 6]);
    f[6..12].copy_from_slice(mac);
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    f[ip] = 0x45;
    f[ip + 2..ip + 4].copy_from_slice(&272u16.to_be_bytes());
    f[ip + 4..ip + 6].copy_from_slice(&3u16.to_be_bytes()); // id
    f[ip + 8] = 64; // ttl
    f[ip + 9] = 17; // udp
    f[ip + 16..ip + 20].copy_from_slice(&[255, 255, 255, 255]);
    let c = checksum(&f[ip..ip + 20]);
    f[ip + 10..ip + 12].copy_from_slice(&c.to_be_bytes());
    f[up..up + 2].copy_from_slice(&68u16.to_be_bytes());
    f[up + 2..up + 4].copy_from_slice(&67u16.to_be_bytes());
    f[up + 4..up + 6].copy_from_slice(&252u16.to_be_bytes());
    f[b] = 1; // bootrequest
    f[b + 1] = 1; // htype: ethernet
    f[b + 2] = 6; // hlen: mac
    f[b + 4..b + 8].copy_from_slice(&DHCP_XID);
    f[b + 10..b + 12].copy_from_slice(&0x8000u16.to_be_bytes()); // broadcast
    f[b + 28..b + 34].copy_from_slice(mac); // chaddr
    f[b + 236..b + 240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    f[b + 240] = 53; // dhcp message type
    f[b + 241] = 1;
    f[b + 242] = 1; // discover
    f[b + 243] = 255;
    let gpa = TXBUF_GPA + (slot * RXBUF_SIZE) as u64;
    poke(txbuf, slot * RXBUF_SIZE, &f);
    tx_post(txring, slot, gpa, f.len());
    regs.wr(TDT, (slot + 1) as u32);
}

// drain filled descriptors; returns (arp_reply_seen, icmp_echo_reply_seen,
// dhcp offer address)
fn drain(
    regs: &Regs,
    st: &mut Ring,
    rxring: PhysAddr,
    rxbuf: PhysAddr,
    gw_mac: &mut [u8; 6],
) -> (bool, bool, Option<[u8; 4]>) {
    let (mut arp_ok, mut icmp_ok) = (false, false);
    let mut udp = None;
    let mut guard = 0;
    while guard < NDESC {
        guard += 1;
        if rx_status(rxring, st.cursor) & 1 == 0 {
            break;
        }
        let len = rx_len(rxring, st.cursor) as usize;
        if (34..=RXBUF_SIZE).contains(&len) {
            let mut pkt = [0u8; 512];
            let take = len.min(pkt.len());
            peek(rxbuf, st.cursor * RXBUF_SIZE, &mut pkt[..take]);
            let etype = ((pkt[12] as u16) << 8) | pkt[13] as u16;
            if etype == 0x0806 && len >= 42 && pkt[20..22] == [0x00, 0x02] {
                gw_mac.copy_from_slice(&pkt[22..28]);
                arp_ok = true;
            }
            if etype == 0x0800 && len >= 14 + 20 + 8 {
                let ihl = ((pkt[14] & 0xF) * 4) as usize;
                if pkt[14 + 9] == 1 && pkt[14 + 20] == 0 {
                    icmp_ok = true;
                }
                if pkt[14 + 9] == 17 && ihl >= 20 && len >= 14 + ihl + 8 + 4 {
                    let b = 14 + ihl + 8; // bootp payload
                    let sport = ((pkt[14 + ihl] as u16) << 8) | pkt[14 + ihl + 1] as u16;
                    let dport = ((pkt[14 + ihl + 2] as u16) << 8) | pkt[14 + ihl + 3] as u16;
                    let reply = b + 240 <= take
                        && sport == 67
                        && dport == 68
                        && pkt[b] == 2
                        && pkt[b + 4..b + 8] == DHCP_XID
                        && pkt[b + 236..b + 240] == [0x63, 0x82, 0x53, 0x63];
                    if reply {
                        let mut i = b + 240;
                        while i + 1 < take {
                            let code = pkt[i];
                            if code == 0 {
                                i += 1;
                                continue;
                            }
                            if code == 255 {
                                break;
                            }
                            let l = pkt[i + 1] as usize;
                            if code == 53 && l == 1 && pkt[i + 2] == 2 {
                                udp = Some([pkt[b + 16], pkt[b + 17], pkt[b + 18], pkt[b + 19]]);
                                break;
                            }
                            i += 2 + l;
                        }
                    }
                }
            }
        }
        rx_clear(rxring, st.cursor);
        regs.wr(RDT, st.cursor as u32);
        st.cursor = (st.cursor + 1) % NDESC;
    }
    (arp_ok, icmp_ok, udp)
}

pub fn smoke() {
    // the dma/intx smoke ran first, inline, and released the device
    let Some(sid) = find_device(0x8086, 0x100E) else {
        return; // no nic — the dma smoke already said so
    };
    let Some(bar) = mmio_bar(sid) else {
        return;
    };
    let Some(ring) = crate::memory::alloc_frame() else {
        return;
    };
    // buffers live in four physically consecutive frames: rx slots 0..4 in
    // the first pair, tx slots 0..4 in the second — slot 2 pokes and dmas
    // past a single frame's 4096-byte end, so singles would corrupt the
    // page that happens to follow
    let Some(bufs) = crate::memory::alloc_frames_aligned(4, 0x1000) else {
        crate::memory::dealloc_frame(ring);
        return;
    };
    let regs_virt = crate::memory::map_mmio(bar, 32);
    let regs = Regs(regs_virt.as_u64() as usize);
    let rxring = ring.start_address();
    let txring = ring.start_address() + 0x100u64;
    let rxbuf_p = bufs[0].start_address();
    let txbuf_p = bufs[2].start_address();

    // mac from the device's reset value; ral/rah must be armed with the
    // valid bit (31): without it the unicast filter drops every reply
    let mut mac = [0u8; 6];
    let ral = regs.rd(RAL);
    let rah = regs.rd(RAH);
    mac[0..4].copy_from_slice(&ral.to_le_bytes());
    mac[4] = rah as u8;
    mac[5] = (rah >> 8) as u8;
    regs.wr(RAL, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
    regs.wr(RAH, u32::from_le_bytes([mac[4], mac[5], 0, 0]) | (1 << 31));
    let mut mb = [b'0'; 17];
    crate::serial_println!("[nic] mac {}", mac_view(&mac, &mut mb));

    let mut dom = match crate::iommu::Domain::new() {
        Some(d) => d,
        None => {
            crate::serial_println!("[nic] no domain — rx smoke skipped");
            free_frames(ring, &bufs);
            return;
        }
    };
    let ok = dom.map(RXRING_GPA, rxring)
        && dom.map(RXBUF_GPA, rxbuf_p)
        && dom.map(RXBUF_GPA + 0x1000, rxbuf_p + 0x1000u64)
        && dom.map(TXBUF_GPA, txbuf_p)
        && dom.map(TXBUF_GPA + 0x1000, txbuf_p + 0x1000u64)
        && dom.attach(sid);
    if !ok {
        crate::serial_println!("[nic] setup failed — rx smoke skipped");
        drop(dom);
        free_frames(ring, &bufs);
        return;
    }
    enable_device(sid);

    // rings: 8 legacy descriptors each (rx at 0x4000, tx at 0x4100 in the
    // same frame), one 2048-byte buffer per descriptor, rdt = 7 hands the
    // whole rx ring to the nic
    unsafe {
        core::ptr::write_bytes(
            crate::memory::phys_to_virt(ring.start_address()).as_mut_ptr::<u8>(),
            0,
            4096,
        );
    }
    for i in 0..NDESC {
        let gpa = if i < 4 {
            RXBUF_GPA + (i * RXBUF_SIZE) as u64
        } else {
            TXBUF_GPA + ((i - 4) * RXBUF_SIZE) as u64
        };
        let p = crate::memory::phys_to_virt(rxring).as_mut_ptr::<u64>();
        unsafe {
            core::ptr::write_volatile(p.add(i * 2), gpa);
        }
    }
    regs.wr(IMC, 0xFFFF_FFFF); // masked until the rings are live
    regs.wr(ICR, 0xFFFF_FFFF); // stale bits out
    regs.wr(CTRL, 0x40); // slu: link up
    regs.wr(RDBAL, RXRING_GPA as u32);
    regs.wr(RDBAH, (RXRING_GPA >> 32) as u32);
    regs.wr(RDLEN, (NDESC * 16) as u32);
    regs.wr(RDH, 0);
    regs.wr(RDT, (NDESC - 1) as u32);
    regs.wr(RCTL, 0x8012); // en (bit 1!) | mpe | bam, 2048-byte buffers
    regs.wr(TDBAL, TXRING_GPA as u32);
    regs.wr(TDBAH, (TXRING_GPA >> 32) as u32);
    regs.wr(TDLEN, (NDESC * 16) as u32);
    regs.wr(TDH, 0);
    regs.wr(TDT, 0);
    regs.wr(TCTL, 0x0003_F0FA); // en | psp | ct 15 | cold 63
    regs.wr(IMS, (1 << 7) | (1 << 4)); // rxt0 | rxdmt0: the intx path lives

    let mut ring_state = Ring::new(&regs);
    let mut gw_mac = [0u8; 6];
    let mut arp_seen = false;
    let mut ping_ok = false;
    let mut ping_sent = false;
    let mut dhcp_sent = false;
    let mut dhcp_start = 0u64;
    let mut udp_offer: Option<[u8; 4]> = None;

    tx_arp_request(&regs, txring, txbuf_p, 0, &mac);
    let _ = tx_done(txring, 0);
    let start = crate::interrupts::tick_count();
    while crate::interrupts::tick_count() < start + 300 {
        x86_64::instructions::interrupts::enable_and_hlt();
        let icr = regs.rd(ICR);
        if icr != 0 {
            regs.wr(ICR, icr); // deassert intx: level lines re-fire after eoi
        }
        let (a, p, u) = drain(&regs, &mut ring_state, rxring, rxbuf_p, &mut gw_mac);
        if a && !arp_seen {
            crate::serial_println!(
                "[nic] arp reply: {}.{}.{}.{} at {}",
                GW_IP[0],
                GW_IP[1],
                GW_IP[2],
                GW_IP[3],
                mac_view(&gw_mac, &mut [b'0'; 17])
            );
            tx_icmp_echo(&regs, txring, txbuf_p, 1, &mac, &gw_mac);
            let _ = tx_done(txring, 1);
            ping_sent = true;
        }
        arp_seen |= a;
        ping_ok |= p;
        if let Some(ip) = u {
            if udp_offer.is_none() {
                udp_offer = Some(ip);
                crate::serial_println!(
                    "[nic] udp: dhcp offer for {}.{}.{}.{} (xid 0x53485a01)",
                    ip[0],
                    ip[1],
                    ip[2],
                    ip[3]
                );
            }
        }
        if ping_ok && !dhcp_sent {
            tx_dhcp_discover(&regs, txring, txbuf_p, 2, &mac);
            let _ = tx_done(txring, 2);
            dhcp_sent = true;
            dhcp_start = crate::interrupts::tick_count();
        }
        // the ping reply may sit in qemu's rx flush queue for up to a
        // second after the last rctl write — do not give up before that;
        // once the discover is out, the offer gets the same slack
        let quiet_after = if dhcp_sent {
            dhcp_start + 150
        } else {
            start + 150
        };
        if udp_offer.is_some()
            || (ping_ok && dhcp_sent && crate::interrupts::tick_count() > quiet_after)
            || (!ping_ok && ping_sent && crate::interrupts::tick_count() > start + 150)
        {
            break;
        }
    }
    if arp_seen && ping_ok {
        crate::serial_println!("[nic] rx ok: arp + icmp echo round trip (host smoke)");
        if udp_offer.is_some() {
            crate::serial_println!("[nic] udp ok: dhcp discover -> offer round trip");
        }
    } else if !arp_seen {
        crate::serial_println!("[nic] arp got no reply (no netdev or link down)");
    } else {
        crate::serial_println!("[nic] arp ok, icmp echo got no reply");
    }
    // leave the nic quiet, then the device free for the next owner (devd):
    // translations must leave the domain before the frames go back
    regs.wr(IMC, 0xFFFF_FFFF);
    regs.wr(RCTL, 0);
    regs.wr(TCTL, 0);
    regs.wr(ICR, 0xFFFF_FFFF);
    drop(dom);
    free_frames(ring, &bufs);
    crate::serial_println!("[nic] down: device released");
}
