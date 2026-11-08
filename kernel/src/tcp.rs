// a real little tcp client: full state machine (syn_sent, established,
// fin_wait_1/2, time_wait, closed), rto retransmissions with exponential
// backoff, mss probe from the peer's syn options, in-order reassembly with
// immediate acks and a fixed two-segment flight. the wire half (rings,
// ip/eth wrap) lives in nic.rs; this is the protocol half the driver-vm
// guest mirrors in c. the deterministic loss demo: the first syn and the
// first data segment go out once with a deliberately bad checksum — slirp
// drops bad-checksum segments (libslirp tcp_input: cksum != 0 -> drop), so
// the rto engine has to save the round: rtx syn 1, rtx data 1, every run.

pub const SERVER_IP: [u8; 4] = [10, 0, 2, 100];
pub const OUR_IP: [u8; 4] = [10, 0, 2, 99];
pub const SERVER_PORT: u16 = 7;
pub const OUR_PORT: u16 = 0xC350;
pub const SEND_TOTAL: usize = 4096;
pub const RECV_TOTAL: usize = 4096;
pub const ADV_WIN: u16 = RECV_TOTAL as u16;

const SYN_SENT: u8 = 1;
const ESTABLISHED: u8 = 2;
const FIN_WAIT_1: u8 = 3;
const FIN_WAIT_2: u8 = 4;
const TIME_WAIT: u8 = 5;
const CLOSED_OK: u8 = 6;

const ERR_NONE: u8 = 0;
const ERR_RST: u8 = 1;
const ERR_TIMEOUT: u8 = 2;

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const ACK: u8 = 0x10;

const RTO_BASE: u32 = 30; // 300 ms at the 100 hz tick
const RTO_LIMIT: u32 = 4; // give up after this many retransmissions
const INFLIGHT_SEGS: usize = 2;

const BANNER: &[u8] = b"shizoley tcp: rto retransmit + state machine + full close ";

pub struct Conn {
    state: u8,
    pub err: u8,
    pub rtx_syn: u32,
    pub rtx_data: u32,
    pub rtx_fin: u32,
    pub echo_bytes: usize,
    pub echo_ok: bool,
    pub echo_at: usize,
    pub established_seen: bool,
    pub acked_seen: bool,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    fin_seq: u32,
    rcv_base: u32,
    rcv_nxt: u32,
    next_off: usize,
    recv_len: usize,
    peer_mss: usize,
    peer_win: u32,
    rto: u32,
    rto_dead: u32,
    tw_dead: u32,
    fin_sent: bool,
    fin_seen: bool,
    syn_built: bool,
    seg0_built: bool,
    sendq: [u8; SEND_TOTAL],
    recvq: [u8; RECV_TOTAL],
}

fn pseudo(src: &[u8; 4], dst: &[u8; 4], tcp_len: usize) -> u32 {
    let mut s = 0u32;
    s += ((src[0] as u32) << 8) | src[1] as u32;
    s += ((src[2] as u32) << 8) | src[3] as u32;
    s += ((dst[0] as u32) << 8) | dst[1] as u32;
    s += ((dst[2] as u32) << 8) | dst[3] as u32;
    s + 6 + tcp_len as u32
}

impl Conn {
    pub fn new(now: u32) -> Conn {
        let mut sendq = [0u8; SEND_TOTAL];
        for (i, b) in sendq.iter_mut().enumerate() {
            *b = BANNER[i % BANNER.len()];
        }
        Conn {
            state: SYN_SENT,
            err: ERR_NONE,
            rtx_syn: 0,
            rtx_data: 0,
            rtx_fin: 0,
            echo_bytes: 0,
            echo_ok: false,
            echo_at: 0,
            established_seen: false,
            acked_seen: false,
            iss: 0x1A2B_3C04,
            snd_una: 0,
            snd_nxt: 0,
            fin_seq: 0,
            rcv_base: 0,
            rcv_nxt: 0,
            next_off: 0,
            recv_len: 0,
            peer_mss: 536,
            peer_win: 0,
            rto: RTO_BASE,
            rto_dead: now + RTO_BASE,
            tw_dead: 0,
            fin_sent: false,
            fin_seen: false,
            syn_built: false,
            seg0_built: false,
            sendq,
            recvq: [0u8; RECV_TOTAL],
        }
    }

    pub fn mss(&self) -> usize {
        self.peer_mss
    }

    // every data byte acked and the fin not sent yet: time to close
    pub fn wants_close(&self) -> bool {
        self.state == ESTABLISHED && !self.fin_sent && self.all_acked()
    }

    pub fn done(&self) -> bool {
        self.state == CLOSED_OK || self.err != ERR_NONE
    }

    pub fn clean(&self) -> bool {
        self.state == CLOSED_OK && self.err == ERR_NONE
    }

    pub fn all_acked(&self) -> bool {
        self.snd_una as usize == self.iss as usize + 1 + SEND_TOTAL
    }

    // build a tcp segment (header + payload) into seg; returns its length.
    // pseudo-header checksum; `corrupt` flips it once for the loss demo
    #[allow(clippy::too_many_arguments)]
    fn build(
        &self,
        seg: &mut [u8; 1536],
        seq: u32,
        ack: u32,
        flags: u8,
        off: usize,
        len: usize,
        with_mss: bool,
        corrupt: bool,
    ) -> usize {
        let hdr = if with_mss { 24 } else { 20 };
        seg[0..2].copy_from_slice(&OUR_PORT.to_be_bytes());
        seg[2..4].copy_from_slice(&SERVER_PORT.to_be_bytes());
        seg[4..8].copy_from_slice(&seq.to_be_bytes());
        seg[8..12].copy_from_slice(&ack.to_be_bytes());
        seg[12] = (hdr as u8 / 4) << 4;
        seg[13] = flags;
        seg[14..16].copy_from_slice(&ADV_WIN.to_be_bytes());
        seg[16..18].copy_from_slice(&[0, 0]); // cksum filled below
        seg[18..20].copy_from_slice(&[0, 0]);
        if with_mss {
            seg[20] = 2;
            seg[21] = 4;
            seg[22..24].copy_from_slice(&1460u16.to_be_bytes());
        }
        seg[hdr..hdr + len].copy_from_slice(&self.sendq[off..off + len]);
        let tcp_len = hdr + len;
        let mut s = pseudo(&OUR_IP, &SERVER_IP, tcp_len);
        let mut i = 0;
        while i + 1 < tcp_len {
            s += ((seg[i] as u32) << 8) | seg[i + 1] as u32;
            i += 2;
        }
        if i < tcp_len {
            s += (seg[i] as u32) << 8;
        }
        while s > 0xFFFF {
            s = (s & 0xFFFF) + (s >> 16);
        }
        let mut c = !(s as u16);
        if corrupt {
            c ^= 0xFFFF; // the one-shot loss demo: slirp drops this
        }
        seg[16..18].copy_from_slice(&c.to_be_bytes());
        hdr + len
    }

    pub fn connect_seg(&mut self, seg: &mut [u8; 1536]) -> usize {
        self.snd_nxt = self.iss + 1;
        let corrupt = !self.syn_built;
        self.syn_built = true;
        self.build(seg, self.iss, 0, SYN, 0, 0, true, corrupt)
    }

    // send the next fresh data segment while the two-segment flight and the
    // peer's advertised window allow it
    pub fn pump(&mut self, seg: &mut [u8; 1536]) -> Option<usize> {
        if self.state != ESTABLISHED || self.next_off >= SEND_TOTAL || self.fin_sent {
            return None;
        }
        let outstanding = (self.snd_nxt - self.snd_una) as usize;
        let win = self.peer_win.max(self.peer_mss as u32) as usize;
        if outstanding >= self.peer_mss * INFLIGHT_SEGS || outstanding + self.peer_mss > win {
            return None;
        }
        let off = self.next_off;
        let len = self.peer_mss.min(SEND_TOTAL - off);
        self.next_off += len;
        self.snd_nxt += len as u32;
        let corrupt = off == 0 && !self.seg0_built;
        self.seg0_built = true;
        Some(self.build(
            seg,
            self.iss + 1 + off as u32,
            self.rcv_nxt,
            ACK,
            off,
            len,
            false,
            corrupt,
        ))
    }

    // the fin: only once every data byte is acked
    pub fn close_seg(&mut self, now: u32, seg: &mut [u8; 1536]) -> usize {
        self.fin_seq = self.snd_nxt;
        self.snd_nxt += 1;
        self.fin_sent = true;
        self.state = FIN_WAIT_1;
        self.rto = RTO_BASE;
        self.rto_dead = now + self.rto;
        self.build(
            seg,
            self.fin_seq,
            self.rcv_nxt,
            FIN | ACK,
            0,
            0,
            false,
            false,
        )
    }

    // rto engine: true when a retransmission is due now
    pub fn rto_due(&self, now: u32) -> bool {
        if now < self.rto_dead {
            return false;
        }
        match self.state {
            SYN_SENT => true,
            ESTABLISHED => self.snd_una < self.snd_nxt,
            FIN_WAIT_1 => self.snd_una < self.snd_nxt,
            _ => false,
        }
    }

    // rebuild the oldest unacked thing; None = gave up (err set)
    pub fn retransmit(&mut self, now: u32, seg: &mut [u8; 1536]) -> Option<usize> {
        self.rto = self.rto.saturating_mul(2);
        self.rto_dead = now + self.rto;
        match self.state {
            SYN_SENT => {
                self.rtx_syn += 1;
                if self.rtx_syn > RTO_LIMIT {
                    self.err = ERR_TIMEOUT;
                    return None;
                }
                Some(self.build(seg, self.iss, 0, SYN, 0, 0, true, false))
            }
            ESTABLISHED if self.snd_una < self.snd_nxt => {
                self.rtx_data += 1;
                if self.rtx_data > RTO_LIMIT {
                    self.err = ERR_TIMEOUT;
                    return None;
                }
                let off = (self.snd_una.wrapping_sub(self.iss).wrapping_sub(1)) as usize;
                if off >= SEND_TOTAL {
                    return None;
                }
                let len = self.peer_mss.min(SEND_TOTAL - off);
                Some(self.build(seg, self.snd_una, self.rcv_nxt, ACK, off, len, false, false))
            }
            FIN_WAIT_1 if self.snd_una < self.snd_nxt => {
                self.rtx_fin += 1;
                if self.rtx_fin > RTO_LIMIT {
                    self.err = ERR_TIMEOUT;
                    return None;
                }
                Some(self.build(
                    seg,
                    self.fin_seq,
                    self.rcv_nxt,
                    FIN | ACK,
                    0,
                    0,
                    false,
                    false,
                ))
            }
            _ => None,
        }
    }

    fn verify_echo(&mut self) {
        self.echo_bytes = self.recv_len;
        for i in 0..self.recv_len {
            if self.recvq[i] != self.sendq[i] {
                self.echo_at = i;
                self.echo_ok = false;
                return;
            }
        }
        self.echo_ok = self.recv_len == SEND_TOTAL;
    }

    // feed one received ethernet frame; may answer with a segment to send
    pub fn input(&mut self, frame: &[u8], now: u32, seg: &mut [u8; 1536]) -> Option<usize> {
        if frame.len() < 54 || frame[12..14] != [0x08, 0x00] {
            return None;
        }
        let ihl = (frame[14] & 0xF) as usize * 4;
        if frame[14 + 9] != 6 || frame.len() < 14 + ihl + 20 {
            return None;
        }
        if frame[26..30] != SERVER_IP {
            return None; // not from the echo server
        }
        let th = 14 + ihl;
        let dport = ((frame[th + 2] as u16) << 8) | frame[th + 3] as u16;
        if dport != OUR_PORT {
            return None;
        }
        let seq = u32::from_be_bytes(frame[th + 4..th + 8].try_into().ok()?);
        let ack = u32::from_be_bytes(frame[th + 8..th + 12].try_into().ok()?);
        let doff = (frame[th + 12] >> 4) as usize * 4;
        if frame.len() < th + doff {
            return None;
        }
        let flags = frame[th + 13];
        let win = u16::from_be_bytes(frame[th + 14..th + 16].try_into().ok()?);
        let ip_total = u16::from_be_bytes(frame[16..18].try_into().ok()?) as usize;
        let plen = ip_total
            .saturating_sub(ihl + 20)
            .min(frame.len().saturating_sub(th + doff));

        if flags & RST != 0 && self.state != CLOSED_OK {
            self.err = ERR_RST;
            return None;
        }

        match self.state {
            SYN_SENT => {
                if flags & (SYN | ACK) == (SYN | ACK) && ack == self.iss + 1 {
                    self.snd_una = self.iss + 1;
                    self.rcv_base = seq + 1;
                    self.rcv_nxt = seq + 1;
                    self.peer_mss = 536;
                    let mut i = th + 20;
                    while i + 1 < th + doff {
                        let kind = frame[i];
                        if kind == 0 {
                            break;
                        }
                        let l = frame[i + 1].max(1) as usize;
                        if kind == 2 && l == 4 && i + 3 < th + doff {
                            self.peer_mss = (((frame[i + 2] as u16) << 8) | frame[i + 3] as u16)
                                .max(64) as usize;
                        }
                        i += l;
                    }
                    self.peer_win = win as u32;
                    self.state = ESTABLISHED;
                    self.established_seen = true;
                    self.rto = RTO_BASE;
                    self.rto_dead = now + self.rto;
                    return Some(self.build(
                        seg,
                        self.iss + 1,
                        self.rcv_nxt,
                        ACK,
                        0,
                        0,
                        false,
                        false,
                    ));
                }
                None
            }
            ESTABLISHED | FIN_WAIT_1 | FIN_WAIT_2 => {
                let mut respond = false;
                if flags & ACK != 0 && ack > self.snd_una && ack <= self.snd_nxt {
                    self.snd_una = ack;
                    if self.snd_una as usize == self.iss as usize + 1 + SEND_TOTAL {
                        self.acked_seen = true;
                    }
                    if self.snd_una < self.snd_nxt {
                        self.rto = RTO_BASE;
                        self.rto_dead = now + self.rto;
                    }
                }
                if plen > 0 {
                    let end = seq + plen as u32;
                    if seq == self.rcv_nxt {
                        let off = (seq - self.rcv_base) as usize;
                        let take = plen.min(RECV_TOTAL.saturating_sub(off));
                        self.recvq[off..off + take]
                            .copy_from_slice(&frame[th + doff..th + doff + take]);
                        if self.recv_len < off + take {
                            self.recv_len = off + take;
                        }
                        self.rcv_nxt = end;
                        respond = true;
                    } else {
                        respond = true; // old tail or gap: re-ack, they go back
                    }
                }
                if flags & FIN != 0 {
                    self.rcv_nxt = seq + plen as u32 + 1;
                    self.fin_seen = true;
                    self.verify_echo();
                    respond = true;
                }
                if self.fin_seen
                    && (self.state == FIN_WAIT_2
                        || (self.state == FIN_WAIT_1 && self.snd_una == self.snd_nxt))
                {
                    self.state = TIME_WAIT;
                    self.tw_dead = now + 2;
                }
                if respond {
                    return Some(self.build(
                        seg,
                        self.snd_nxt,
                        self.rcv_nxt,
                        ACK,
                        0,
                        0,
                        false,
                        false,
                    ));
                }
                None
            }
            TIME_WAIT => {
                if flags & FIN != 0 {
                    return Some(self.build(
                        seg,
                        self.snd_nxt,
                        self.rcv_nxt,
                        ACK,
                        0,
                        0,
                        false,
                        false,
                    ));
                }
                None
            }
            _ => None,
        }
    }

    pub fn tick(&mut self, now: u32) {
        if self.state == TIME_WAIT && now >= self.tw_dead {
            self.state = CLOSED_OK;
        }
    }
}
