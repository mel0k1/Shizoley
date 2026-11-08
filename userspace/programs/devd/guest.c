// guest side of the driver-vm round: a real e1000 driver running in the
// guest — tx AND rx. the nic dmas the descriptor rings straight out of
// guest ram (gpa addresses, translated by the vt-d domain that mirrors
// the guest ept), and the bar reads go through the ept uc leaf (emt 0) —
// neither the rings nor the bar are cacheable by any guest pat trick.
// interrupts: the monitor routes the nic's host vector into the guest
// (guest_pi), the kernel posts it (pir on apicv, entry injection without),
// and the idt handler here only drains the rx ring into a small packet
// queue — the main loop does all the protocol work. the stack is a full
// client round, no host pinger needed: arp resolve for the gateway, icmp
// echo to it, then a REAL tcp client (10.0.2.100:7, qemu user-net echo
// via guestfwd cmd:cat): syn_sent/established/fin_wait_1/fin_wait_2/
// time_wait state machine, rto retransmissions with exponential backoff,
// mss from the syn-ack options, echo bytes verified. on top the guest
// serves a udp echo on port 4242 (hostfwd'ed on kvm: the host sends to
// 127.0.0.1:4242 and the guest answers) — nr 9 reports it. the wire twin
// of this tcp lives in kernel tcp.rs and is tcg-verified in the host
// smoke; the first syn and the first data segment go out once with a bad
// checksum (slirp drops those) so the rto engine proves itself: rtx
// syn 1 data 1 fin 0, every run. results travel to the monitor over
// vmcall: nr 2 = the status read, nr 3 = the tx report, nr 4 = ready
// with the mac, nr 5 = link status, nr 6 = an rx packet report, nr 7 =
// the arp resolve, nr 8 = the ping echo reply, nr 9 = the udp echo
// (rsi = payload bytes, rdx = sender ip<<32 | sender port), nr 10 = tcp
// connect, nr 11 = all data acked, nr 12 = an rto retransmission,
// nr 13 = the echo verdict, nr 14 = closed.
typedef unsigned long u64;
typedef unsigned int u32;
typedef unsigned short u16;
typedef unsigned char u8;

#define BAR 0xFEBC0000UL
#define REG(off) (*(volatile u32 *)(BAR + (off)))

#define CTRL 0x0000
#define STATUS 0x0008
#define ICR 0x00C0
#define IMS 0x00D0
#define IMC 0x00D8
#define RCTL 0x0100
#define TCTL 0x0400
#define RDBAL 0x2800
#define RDBAH 0x2804
#define RDLEN 0x2808
#define RDH 0x2810
#define RDT 0x2818
#define TDBAL 0x3800
#define TDBAH 0x3804
#define TDLEN 0x3808
#define TDH 0x3810
#define TDT 0x3818
#define RAL 0x5400
#define RAH 0x5404

#define RXRING 0x10000UL
#define RXBUF 0x11000UL
#define TXRING 0x32000UL
#define TXBUF 0x33000UL
#define NRX 64
#define NTX 4
#define BUFSZ 2048
#define IDT 0x7000UL
#define VEC 0xC008UL
#define NUDGE 0x22

#define OUR_IP 0x6302000AUL    /* 10.0.2.99, wire order */
#define GW_IP 0x0202000AUL     /* 10.0.2.2, wire order */
#define SERVER_IP 0x6402000AUL /* 10.0.2.100, wire order */
#define SERVER_PORT 7
#define OUR_PORT 0xC350 /* 50000 */
#define ECHO_PORT 4242  /* the udp echo service */

/* guest ram beyond the rings: tcp workspace (grown pages) */
#define SENDQ 0x36000UL /* 4096 bytes of payload the echo must return */
#define RECVQ 0x37000UL /* 4096 bytes of echoed payload */
#define PKTQ 0x38000UL  /* 4 slots x 2048: isr -> main loop frames */
#define PKTS 4
#define SEND_TOTAL 4096
#define RECV_TOTAL 4096

#define T_CLOSED 0
#define T_SYN_SENT 1
#define T_EST 2
#define T_FW1 3
#define T_FW2 4
#define T_TW 5
#define T_DONE 6

#define FL_FIN 0x01
#define FL_SYN 0x02
#define FL_RST 0x04
#define FL_ACK 0x10

#define E_NONE 0
#define E_RST 1
#define E_TIMEOUT 2

static inline u64 hyper3(u64 nr, u64 a1, u64 a2) {
    u64 r;
    asm volatile("vmcall" : "=a"(r) : "a"(nr), "S"(a1), "d"(a2) : "memory");
    return r;
}

static inline void hyper(u64 nr, u64 a1, u64 a2) {
    hyper3(nr, a1, a2);
}

extern void nic_isr(void);
extern void nudge_isr(void);
__asm__(".text\n"
        "nic_isr:\n"
        "    push %rax\n"
        "    push %rcx\n"
        "    push %rdx\n"
        "    push %rsi\n"
        "    push %rdi\n"
        "    push %r8\n"
        "    push %r9\n"
        "    push %r10\n"
        "    push %r11\n"
        "    xor %edi, %edi\n"
        "    call irq_body\n"
        "    pop %r11\n"
        "    pop %r10\n"
        "    pop %r9\n"
        "    pop %r8\n"
        "    pop %rdi\n"
        "    pop %rsi\n"
        "    pop %rdx\n"
        "    pop %rcx\n"
        "    pop %rax\n"
        "    iretq\n"
        "nudge_isr:\n"
        "    push %rax\n"
        "    push %rcx\n"
        "    push %rdx\n"
        "    push %rsi\n"
        "    push %rdi\n"
        "    push %r8\n"
        "    push %r9\n"
        "    push %r10\n"
        "    push %r11\n"
        "    mov $1, %edi\n"
        "    call irq_body\n"
        "    pop %r11\n"
        "    pop %r10\n"
        "    pop %r9\n"
        "    pop %r8\n"
        "    pop %rdi\n"
        "    pop %rsi\n"
        "    pop %rdx\n"
        "    pop %rcx\n"
        "    pop %rax\n"
        "    iretq\n");

static u32 irq_count;
static u32 jiffies; /* advanced by the monitor's nudge vector only */
static u32 rx_cursor;
static u32 tx_slot;
static u16 ip_id;
static u64 our_mac;
static u64 gw_mac;
static u32 gw_ok;
static u32 lsc_seen;
static u32 pkq_head;
static u32 pkq_tail;
static u32 pkq_len[PKTS];
static u32 pkq_drop;
static u32 ping_replied;

static void idt_set(u64 v, u64 handler) {
    volatile u64 *e = (volatile u64 *)(IDT + v * 16);
    e[0] = (handler & 0xFFFF) | (0x8UL << 16) | (0x8EUL << 40) |
           ((handler & 0xFFFF0000UL) << 32);
    e[1] = handler >> 32;
}

static void lidt(void) {
    struct {
        u16 limit;
        u64 base;
    } __attribute__((packed)) idtr = {4096, IDT};
    asm volatile("lidt %0" : : "m"(idtr));
}

static u16 be16(volatile const u8 *p) {
    return (u16)((p[0] << 8) | p[1]);
}

static u32 be32(volatile const u8 *p) {
    return ((u32)p[0] << 24) | ((u32)p[1] << 16) | ((u32)p[2] << 8) | p[3];
}

static void put16(volatile u8 *p, u16 v) {
    p[0] = (u8)(v >> 8);
    p[1] = (u8)v;
}

static void put32(volatile u8 *p, u32 v) {
    p[0] = (u8)(v >> 24);
    p[1] = (u8)(v >> 16);
    p[2] = (u8)(v >> 8);
    p[3] = (u8)v;
}

static u16 load16(const u8 *p) {
    return (u16)((p[0] << 8) | p[1]);
}

static u32 load32(const u8 *p) {
    return ((u32)p[0] << 24) | ((u32)p[1] << 16) | ((u32)p[2] << 8) | p[3];
}

/* internet checksum over a wire-order span */
static u16 cksum(volatile const u8 *p, u32 n) {
    u32 sum = 0;
    u32 i = 0;
    while (i + 1 < n) {
        sum += ((u32)p[i] << 8) | p[i + 1];
        i += 2;
    }
    if (i < n) {
        sum += (u32)p[i] << 8;
    }
    while (sum > 0xFFFF) {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    return (u16)~sum;
}

/* checksum with a pseudo-header (src, dst, proto, len) over a span */
static u16 pseudo_cksum(const u8 *seg, u32 len, u32 src_ip, u32 dst_ip,
                        u32 proto) {
    u32 sum = (src_ip >> 16) + (src_ip & 0xFFFF) + (dst_ip >> 16) +
              (dst_ip & 0xFFFF) + proto + len;
    u32 i = 0;
    while (i + 1 < len) {
        sum += ((u32)seg[i] << 8) | seg[i + 1];
        i += 2;
    }
    if (i < len) {
        sum += (u32)seg[i] << 8;
    }
    while (sum > 0xFFFF) {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    return (u16)~sum;
}

/* post an already-built frame into the next tx slot and wait it out */
static void tx_frame(const u8 *f, u16 len) {
    volatile u8 *d = (volatile u8 *)(TXRING + (u64)tx_slot * 16);
    volatile u8 *b = (volatile u8 *)(TXBUF + (u64)tx_slot * BUFSZ);
    u64 gpa = TXBUF + (u64)tx_slot * BUFSZ;
    for (u32 i = 0; i < len; ++i) {
        b[i] = f[i];
    }
    d[0] = (u8)gpa;
    d[1] = (u8)(gpa >> 8);
    d[2] = (u8)(gpa >> 16);
    d[3] = (u8)(gpa >> 24);
    d[4] = d[5] = d[6] = d[7] = 0;
    d[8] = (u8)(len & 0xFF);
    d[9] = (u8)(len >> 8);
    d[10] = 0;
    d[11] = 0x0B; /* eop | ifcs | rs */
    d[12] = 0;
    d[13] = 0;
    d[14] = 0;
    d[15] = 0;
    REG(TDT) = ((tx_slot + 1) % NTX);
    for (long spin = 0; spin < 4000000; ++spin) {
        if (d[12] & 1) {
            break;
        }
    }
    tx_slot = (tx_slot + 1) % NTX;
}

/* ip+eth wrap for a ready-made tcp segment */
static void tx_tcp(const u8 *seg, u16 seg_len) {
    u8 f[1536];
    u16 total = (u16)(34 + seg_len);
    for (int i = 0; i < 6; ++i) {
        f[i] = ((volatile const u8 *)&gw_mac)[i];
        f[6 + i] = ((volatile const u8 *)&our_mac)[i];
    }
    put16(f + 12, 0x0800);
    f[14] = 0x45;
    put16(f + 16, (u16)(total - 14));
    put16(f + 18, 7);
    f[22] = 64;
    f[23] = 6;
    put32(f + 26, OUR_IP);
    put32(f + 30, SERVER_IP);
    put16(f + 24, cksum(f + 14, 20));
    for (u32 i = 0; i < seg_len; ++i) {
        f[34 + i] = seg[i];
    }
    tx_frame(f, total);
}

/* arp request: who has the gateway */
static void arp_request_send(void) {
    u8 f[42];
    for (int i = 0; i < 42; ++i) {
        f[i] = 0;
    }
    for (int i = 0; i < 6; ++i) {
        f[i] = 0xFF;
        f[6 + i] = ((volatile const u8 *)&our_mac)[i];
        f[22 + i] = ((volatile const u8 *)&our_mac)[i];
    }
    put16(f + 12, 0x0806);
    put16(f + 14, 1);
    put16(f + 16, 0x0800);
    f[18] = 6;
    f[19] = 4;
    put16(f + 20, 1);
    put32(f + 28, OUR_IP);
    put32(f + 38, GW_IP);
    tx_frame(f, 42);
}

/* icmp echo request to the gateway */
static void ping_send(u16 seq) {
    u8 f[63];
    for (int i = 0; i < 63; ++i) {
        f[i] = 0;
    }
    for (int i = 0; i < 6; ++i) {
        f[i] = ((volatile const u8 *)&gw_mac)[i];
        f[6 + i] = ((volatile const u8 *)&our_mac)[i];
    }
    put16(f + 12, 0x0800);
    f[14] = 0x45;
    put16(f + 16, 49);
    put16(f + 18, 2);
    f[22] = 64;
    f[23] = 1;
    put32(f + 26, OUR_IP);
    put32(f + 30, GW_IP);
    f[34] = 8; /* echo request */
    f[36] = 0;
    f[37] = 2;
    put16(f + 38, seq);
    const char *pl = "paparaci rx smoke";
    for (int i = 0; i < 17; ++i) {
        f[42 + i] = (u8)pl[i];
    }
    put16(f + 24, cksum(f + 14, 20));
    put16(f + 36, cksum(f + 34, 29));
    tx_frame(f, 63);
}

/* answer an arp request for our address (the gateway probing us) */
static void do_arp_request(volatile const u8 *pkt) {
    if (be16(pkt + 20) != 1 || be32(pkt + 38) != OUR_IP) {
        return;
    }
    u8 f[42];
    for (int i = 0; i < 42; ++i) {
        f[i] = 0;
    }
    for (int i = 0; i < 6; ++i) {
        f[i] = pkt[6 + i];
        f[6 + i] = ((volatile const u8 *)&our_mac)[i];
        f[22 + i] = ((volatile const u8 *)&our_mac)[i];
        f[32 + i] = pkt[22 + i];
    }
    put16(f + 12, 0x0806);
    put16(f + 14, 1);
    put16(f + 16, 0x0800);
    f[18] = 6;
    f[19] = 4;
    put16(f + 20, 2);
    put32(f + 28, OUR_IP);
    put32(f + 38, be32(pkt + 28));
    tx_frame(f, 42);
}

/* answer an icmp echo request aimed at us */
static void do_icmp_request(volatile const u8 *pkt, u16 len) {
    u32 ihl = (u32)(pkt[14] & 0xF) * 4;
    if (pkt[23] != 1 || len < 14 + ihl + 8) {
        return;
    }
    volatile const u8 *ic = pkt + 14 + ihl;
    if (ic[0] != 8) {
        return;
    }
    u8 f[1600];
    u16 n = len;
    if (n > 1500) {
        n = 1500;
    }
    for (u16 i = 0; i < n; ++i) {
        f[i] = pkt[i];
    }
    for (int i = 0; i < 6; ++i) {
        f[i] = pkt[6 + i];
        f[6 + i] = ((volatile const u8 *)&our_mac)[i];
    }
    put16(f + 12, 0x0800);
    f[14] = 0x45;
    put16(f + 16, (u16)(n - 14));
    put16(f + 18, be16(pkt + 18));
    f[20] = 0;
    f[21] = 0;
    f[22] = 64;
    f[23] = 1;
    put32(f + 26, OUR_IP);
    put32(f + 30, be32(pkt + 26));
    put16(f + 24, 0);
    put16(f + 24, cksum(f + 14, 20));
    f[14 + ihl] = 0;
    put16(f + 14 + ihl + 2, 0);
    put16(f + 14 + ihl + 2, cksum(f + 14 + ihl, (u32)(n - 14 - ihl)));
    tx_frame(f, n);
    hyper(8, be16(ic + 6), 1);
}

/* udp echo service on 4242: the datagram comes back with swapped ports */
static void do_udp_echo(volatile const u8 *pkt, u16 len) {
    u32 ihl = (u32)(pkt[14] & 0xF) * 4;
    if (len < 14 + ihl + 8) {
        return;
    }
    volatile const u8 *u = pkt + 14 + ihl;
    u16 ulen = be16(u + 4);
    u16 sport = be16(u);
    u16 dport = be16(u + 2);
    if (dport != ECHO_PORT || ulen < 8 || ulen > BUFSZ ||
        14u + ihl + ulen > (u32)len) {
        return;
    }
    u32 dst_ip = be32(pkt + 26); /* the sender's address, wire order */
    u8 f[1600];
    u32 n = 14u + 20u + ulen;
    for (u32 i = 0; i < n; ++i) {
        f[i] = 0;
    }
    for (int i = 0; i < 6; ++i) {
        f[i] = pkt[6 + i]; /* dst = sender mac */
        f[6 + i] = ((volatile const u8 *)&our_mac)[i];
    }
    put16(f + 12, 0x0800);
    f[14] = 0x45;
    put16(f + 16, (u16)(20 + ulen));
    put16(f + 18, ip_id++);
    f[22] = 64;
    f[23] = 17;
    put32(f + 26, OUR_IP);
    put32(f + 30, dst_ip);
    put16(f + 24, cksum(f + 14, 20));
    u8 *ru = f + 34;
    for (u16 i = 0; i < ulen; ++i) {
        ru[i] = u[i];
    }
    put16(ru, dport);
    put16(ru + 2, sport);
    put16(ru + 4, ulen);
    put16(ru + 6, pseudo_cksum(ru, ulen, OUR_IP, dst_ip, 17));
    tx_frame(f, (u16)n);
    hyper(9, ulen - 8, ((u64)dst_ip << 32) | sport);
}

/* ---- the tcp client: the c twin of kernel tcp.rs ---- */

static u8 tstate;
static u8 terr;
static u32 iss;
static u32 snd_una;
static u32 snd_nxt;
static u32 fin_seq;
static u32 rcv_base;
static u32 rcv_nxt;
static u32 next_off;
static u32 recv_len;
static u32 peer_mss;
static u32 peer_win;
static u32 rto;
static u32 rto_dead;
static u32 tw_dead;
static u32 rtx_syn;
static u32 rtx_data;
static u32 rtx_fin;
static u32 echo_at;
static u8 fin_sent;
static u8 fin_seen;
static u8 syn_built;
static u8 seg0_built;
static u8 acked_seen;
static u8 echo_ok;
static u8 established_seen;
static volatile u8 *SEND = (volatile u8 *)SENDQ;
static volatile u8 *RECV = (volatile u8 *)RECVQ;

static const char banner[] =
    "shizoley tcp: rto retransmit + state machine + full close ";

static u32 min_u32(u32 a, u32 b) {
    return a < b ? a : b;
}

/* build a tcp segment into seg; returns its length */
static u32 seg_build(u8 *seg, u32 seq, u32 ack, u8 flags, u32 off, u32 len,
                     u32 with_mss, u32 corrupt) {
    u32 hdr = with_mss ? 24 : 20;
    put16(seg + 0, OUR_PORT);
    put16(seg + 2, SERVER_PORT);
    put32(seg + 4, seq);
    put32(seg + 8, ack);
    seg[12] = (u8)((hdr / 4) << 4);
    seg[13] = flags;
    put16(seg + 14, RECV_TOTAL);
    put16(seg + 16, 0);
    put16(seg + 18, 0);
    if (with_mss) {
        seg[20] = 2;
        seg[21] = 4;
        put16(seg + 22, 1460);
    }
    for (u32 i = 0; i < len; ++i) {
        seg[hdr + i] = SEND[off + i];
    }
    u32 n = hdr + len;
    u16 c = pseudo_cksum(seg, n, OUR_IP, SERVER_IP, 6);
    if (corrupt) {
        c = (u16)(c ^ 0xFFFF); /* the one-shot loss demo: slirp drops this */
    }
    put16(seg + 16, c);
    return n;
}

static void conn_start(u32 now) {
    for (u32 i = 0; i < SEND_TOTAL; ++i) {
        SEND[i] = (u8)banner[i % (sizeof(banner) - 1)];
    }
    tstate = T_SYN_SENT;
    terr = E_NONE;
    iss = 0x1A2B3C04UL;
    snd_una = 0;
    snd_nxt = 0;
    fin_seq = 0;
    rcv_base = 0;
    rcv_nxt = 0;
    next_off = 0;
    recv_len = 0;
    peer_mss = 536;
    peer_win = 0;
    rto = 1; /* one nudge period (~300 ms) */
    rto_dead = now + rto;
    tw_dead = 0;
    rtx_syn = 0;
    rtx_data = 0;
    rtx_fin = 0;
    echo_at = 0;
    fin_sent = 0;
    fin_seen = 0;
    syn_built = 0;
    seg0_built = 0;
    acked_seen = 0;
    echo_ok = 0;
    established_seen = 0;
    u8 seg[1536];
    u32 n = seg_build(seg, iss, 0, FL_SYN, 0, 0, 1, syn_built == 0);
    syn_built = 1;
    snd_nxt = iss + 1;
    tx_tcp(seg, (u16)n);
}

/* the next fresh data segment while the two-segment flight allows */
static u32 conn_pump(u8 *seg) {
    if (tstate != T_EST || next_off >= SEND_TOTAL || fin_sent) {
        return 0;
    }
    u32 outstanding = snd_nxt - snd_una;
    u32 win = peer_win > peer_mss ? peer_win : peer_mss;
    if (outstanding >= peer_mss * 2 || outstanding + peer_mss > win) {
        return 0;
    }
    u32 off = next_off;
    u32 len = min_u32(peer_mss, SEND_TOTAL - off);
    next_off += len;
    snd_nxt += len;
    u32 corrupt = (off == 0 && seg0_built == 0);
    seg0_built = 1;
    return seg_build(seg, iss + 1 + off, rcv_nxt, FL_ACK, off, len, 0, corrupt);
}

static u32 conn_close(u32 now, u8 *seg) {
    fin_seq = snd_nxt;
    snd_nxt += 1;
    fin_sent = 1;
    tstate = T_FW1;
    rto = 1;
    rto_dead = now + rto;
    return seg_build(seg, fin_seq, rcv_nxt, FL_FIN | FL_ACK, 0, 0, 0, 0);
}

static u32 conn_rto_due(u32 now) {
    if (now < rto_dead) {
        return 0;
    }
    if (tstate == T_SYN_SENT) {
        return 1;
    }
    if (tstate == T_EST) {
        return snd_una < snd_nxt;
    }
    if (tstate == T_FW1) {
        return snd_una < snd_nxt;
    }
    return 0;
}

/* rebuild the oldest unacked thing; 0 = gave up (terr set) */
static u32 conn_retransmit(u32 now, u8 *seg) {
    rto = rto * 2;
    rto_dead = now + rto;
    if (tstate == T_SYN_SENT) {
        rtx_syn += 1;
        if (rtx_syn > 4) {
            terr = E_TIMEOUT;
            return 0;
        }
        return seg_build(seg, iss, 0, FL_SYN, 0, 0, 1, 0);
    }
    if (tstate == T_EST && snd_una < snd_nxt) {
        rtx_data += 1;
        if (rtx_data > 4) {
            terr = E_TIMEOUT;
            return 0;
        }
        u32 off = snd_una - iss - 1;
        if (off >= SEND_TOTAL) {
            return 0;
        }
        u32 len = min_u32(peer_mss, SEND_TOTAL - off);
        return seg_build(seg, snd_una, rcv_nxt, FL_ACK, off, len, 0, 0);
    }
    if (tstate == T_FW1 && snd_una < snd_nxt) {
        rtx_fin += 1;
        if (rtx_fin > 4) {
            terr = E_TIMEOUT;
            return 0;
        }
        return seg_build(seg, fin_seq, rcv_nxt, FL_FIN | FL_ACK, 0, 0, 0, 0);
    }
    return 0;
}

static void verify_echo(void) {
    for (u32 i = 0; i < recv_len; ++i) {
        if (RECV[i] != SEND[i]) {
            echo_at = i;
            echo_ok = 0;
            return;
        }
    }
    echo_ok = (recv_len == SEND_TOTAL);
}

/* feed one received frame (eth+ip+tcp) */
static void conn_input(const u8 *f, u32 flen, u32 now, u8 *seg) {
    if (flen < 54 || load16(f + 12) != 0x0800) {
        return;
    }
    u32 ihl = (u32)(f[14] & 0xF) * 4;
    if (f[14 + 9] != 6 || flen < 14 + ihl + 20) {
        return;
    }
    if (load32(f + 26) != SERVER_IP) {
        return;
    }
    u32 th = 14 + ihl;
    u16 dport = load16(f + th + 2);
    if (dport != OUR_PORT) {
        return;
    }
    u32 seq = load32(f + th + 4);
    u32 ack = load32(f + th + 8);
    u32 doff = (u32)(f[th + 12] >> 4) * 4;
    if (flen < th + doff) {
        return;
    }
    u8 flags = f[th + 13];
    u32 win = load16(f + th + 14);
    u32 plen = load16(f + 16);
    plen = (plen > ihl + 20) ? plen - ihl - 20 : 0;
    if (plen > flen - th - doff) {
        plen = flen - th - doff;
    }
    if ((flags & FL_RST) && tstate != T_DONE) {
        terr = E_RST;
        return;
    }
    if (tstate == T_SYN_SENT) {
        if ((flags & (FL_SYN | FL_ACK)) == (FL_SYN | FL_ACK) &&
            ack == iss + 1) {
            snd_una = iss + 1;
            rcv_base = seq + 1;
            rcv_nxt = seq + 1;
            peer_mss = 536;
            u32 i = th + 20;
            while (i + 1 < th + doff) {
                u8 kind = f[i];
                if (kind == 0) {
                    break;
                }
                u32 l = f[i + 1] ? f[i + 1] : 1;
                if (kind == 2 && l == 4 && i + 3 < th + doff) {
                    u32 m = load16(f + i + 2);
                    peer_mss = m > 64 ? m : 64;
                }
                i += l;
            }
            peer_win = win;
            tstate = T_EST;
            established_seen = 1;
            rto = 1;
            rto_dead = now + rto;
            u32 n = seg_build(seg, iss + 1, rcv_nxt, FL_ACK, 0, 0, 0, 0);
            tx_tcp(seg, (u16)n);
            hyper(10, 1, peer_mss);
        }
        return;
    }
    if (tstate == T_EST || tstate == T_FW1 || tstate == T_FW2) {
        u32 respond = 0;
        if ((flags & FL_ACK) && ack > snd_una && ack <= snd_nxt) {
            snd_una = ack;
            if (snd_una == iss + 1 + SEND_TOTAL) {
                acked_seen = 1;
                hyper(11, SEND_TOTAL, 0);
            }
            if (snd_una < snd_nxt) {
                rto = 1;
                rto_dead = now + rto;
            }
        }
        if (plen > 0) {
            u32 end = seq + plen;
            if (seq == rcv_nxt) {
                u32 off = seq - rcv_base;
                if (off < RECV_TOTAL) {
                    u32 take = min_u32(plen, RECV_TOTAL - off);
                    for (u32 i = 0; i < take; ++i) {
                        RECV[off + i] = f[th + doff + i];
                    }
                    if (recv_len < off + take) {
                        recv_len = off + take;
                    }
                }
                rcv_nxt = end;
                respond = 1;
            } else {
                respond = 1; /* old tail or gap: re-ack, they go back */
            }
        }
        if (flags & FL_FIN) {
            rcv_nxt = seq + plen + 1;
            fin_seen = 1;
            verify_echo();
            hyper(13, recv_len, echo_ok ? 1 : echo_at + 2);
            respond = 1;
        }
        if (fin_seen &&
            (tstate == T_FW2 || (tstate == T_FW1 && snd_una == snd_nxt))) {
            tstate = T_TW;
            tw_dead = now + 2;
        }
        if (respond) {
            u32 n = seg_build(seg, snd_nxt, rcv_nxt, FL_ACK, 0, 0, 0, 0);
            tx_tcp(seg, (u16)n);
        }
        return;
    }
    if (tstate == T_TW && (flags & FL_FIN)) {
        u32 n = seg_build(seg, snd_nxt, rcv_nxt, FL_ACK, 0, 0, 0, 0);
        tx_tcp(seg, (u16)n);
    }
}

static void conn_tick(u32 now) {
    if (tstate == T_TW && now >= tw_dead) {
        tstate = T_DONE;
    }
}

/* the tcp engine run once per main-loop pass */
static void tcp_engine(u8 *seg) {
    u32 now = jiffies;
    u32 n = conn_pump(seg);
    if (n) {
        tx_tcp(seg, (u16)n);
    }
    if (tstate == T_EST && !fin_sent && snd_una == iss + 1 + SEND_TOTAL &&
        next_off >= SEND_TOTAL) {
        n = conn_close(now, seg);
        tx_tcp(seg, (u16)n);
    }
    if (conn_rto_due(now) && !terr && tstate != T_DONE) {
        n = conn_retransmit(now, seg);
        if (n) {
            u32 phase = tstate == T_SYN_SENT ? 1 : (tstate == T_EST ? 2 : 3);
            hyper(12, phase, 0);
            tx_tcp(seg, (u16)n);
        }
    }
    conn_tick(now);
}

/* ---- frame processing (main-loop half) ---- */

static void process_frame(volatile const u8 *pkt, u16 len, u8 *seg) {
    u16 et = be16(pkt + 12);
    if (et == 0x0800) {
        u32 src6 = be32(pkt + 26);
        hyper(6, (u64)len, (0x0800UL << 32) | src6);
    }
    if (et == 0x0806) {
        if (be16(pkt + 20) == 2 && be32(pkt + 28) == GW_IP) {
            gw_mac = 0;
            for (int i = 0; i < 6; ++i) {
                ((u8 *)&gw_mac)[i] = pkt[22 + i];
            }
            if (!gw_ok) {
                gw_ok = 1;
                hyper(7, gw_mac & 0xFFFFFFFF, gw_mac >> 32);
            }
        } else {
            do_arp_request(pkt);
        }
        return;
    }
    if (et != 0x0800) {
        return;
    }
    u32 ihl = (u32)(pkt[14] & 0xF) * 4;
    if (pkt[23] == 1) {
        volatile const u8 *ic = pkt + 14 + ihl;
        if (ic[0] == 0 && be32(pkt + 26) == GW_IP) {
            /* the reply to our own ping */
            ping_replied = 1;
            hyper(8, be16(ic + 6), 0);
            return;
        }
        do_icmp_request(pkt, len);
        return;
    }
    if (pkt[23] == 17) {
        do_udp_echo(pkt, len);
        return;
    }
    if (pkt[23] == 6) {
        u8 f[1600];
        u32 n = len;
        if (n > 1600) {
            n = 1600;
        }
        for (u32 i = 0; i < n; ++i) {
            f[i] = pkt[i];
        }
        conn_input(f, n, jiffies, seg);
    }
}

/* the idt body: bump the timer clock on nudges, deassert the level line,
 * then drain everything the nic filled into the packet queue */
void irq_body(u32 is_nudge) {
    if (is_nudge) {
        jiffies += 1;
    }
    u32 icr = REG(ICR);
    REG(ICR) = icr;
    irq_count++;
    if (icr & 4) {
        lsc_seen = 1;
    }
    if (!(icr & 0x90)) {
        return; /* rxt0 | rxdmt0 */
    }
    for (int n = 0; n < NRX; ++n) {
        volatile u8 *d = (volatile u8 *)(RXRING + (u64)rx_cursor * 16);
        u8 st = d[12];
        if (!(st & 1)) {
            break; /* dd clear: nothing more filled */
        }
        u16 len = (u16)(d[8] | (d[9] << 8));
        if (st & 2 && len >= 34 && len <= BUFSZ) { /* eop, sane size */
            volatile u8 *slot =
                (volatile u8 *)(PKTQ + (u64)pkq_head * BUFSZ);
            volatile const u8 *pkt =
                (volatile const u8 *)(RXBUF + (u64)rx_cursor * BUFSZ);
            if ((pkq_head + 1) % PKTS != pkq_tail) {
                for (u32 i = 0; i < len; ++i) {
                    slot[i] = pkt[i];
                }
                pkq_len[pkq_head] = len;
                pkq_head = (pkq_head + 1) % PKTS;
            } else {
                pkq_drop += 1; /* tcp retransmission will cover it */
            }
        }
        d[12] = 0; /* hand the descriptor back */
        REG(RDT) = rx_cursor;
        rx_cursor = (rx_cursor + 1) % NRX;
    }
}

/* main-loop phases: arp resolve, ping, tcp, serve */
#define P_ARP 0
#define P_PING 1
#define P_TCP 2
#define P_SERVE 3

void guest_main(void) {
    u32 status = REG(STATUS);
    hyper(2, status, 8);
    u64 vec = *(volatile u64 *)VEC;

    /* mac: the device's reset value (qemu loads it from the netdev);
     * ral/rah must be armed with the valid bit (31) — without it the
     * unicast filter drops every reply addressed to us */
    u32 ral = REG(RAL);
    u32 rah = REG(RAH) & 0xFFFF;
    our_mac = (u64)ral | ((u64)rah << 32);
    if (!our_mac) {
        our_mac = 0x563412005452UL; /* 52:54:00:12:34:56 */
    }
    REG(RAL) = (u32)our_mac;
    REG(RAH) = (u32)(our_mac >> 32) | 0x80000000u;

    REG(CTRL) = 0x40;         // slu
    REG(TDBAL) = (u32)TXRING; // ring in guest ram: the nic's dma reaches it
                              // only through the guest's vt-d domain
    REG(TDBAH) = 0;
    REG(TDLEN) = NTX * 16;
    REG(TDH) = 0;
    REG(TDT) = 0;
    REG(TCTL) = 0x00040F0AUL; // en | psp | ct | cold

    volatile u8 *desc = (volatile u8 *)TXRING;
    u8 *pkt = (u8 *)TXBUF;
    for (int i = 0; i < 64; ++i) {
        pkt[i] = (u8)i;
    }

    desc[0] = (u8)(TXBUF & 0xFF); // buffer addr, little-endian gpa
    desc[1] = (u8)((TXBUF >> 8) & 0xFF);
    desc[2] = (u8)((TXBUF >> 16) & 0xFF);
    desc[3] = (u8)((TXBUF >> 24) & 0xFF);
    desc[4] = 0;
    desc[5] = 0;
    desc[6] = 0;
    desc[7] = 0;
    desc[8] = 64 & 0xFF; // length
    desc[9] = (64 >> 8) & 0xFF;
    desc[10] = 0;    // cso
    desc[11] = 0x0B; // eop | ifcs | rs
    desc[12] = 0;    // status: the nic sets dd (bit 0) over dma
    desc[13] = 0;    // errors
    desc[14] = 0;
    desc[15] = 0;

    REG(IMC) = ~0u;
    REG(ICR) = 0xFFFFFFFFu; // stale causes out
    u32 irq = 0;
    if (vec) {
        idt_set(vec, (u64)nic_isr);
        idt_set(NUDGE, (u64)nudge_isr); /* the monitor's wake + clock */
        lidt();
        asm volatile("sti");
        REG(IMS) = 1; // txdw: the completion interrupt
    }
    REG(TDT) = 1; // kick
    if (vec) {
        for (long spin = 0; spin < 8000000; ++spin) {
            irq = irq_count;
            if (irq) {
                break;
            }
        }
    } else {
        for (long spin = 0; spin < 8000000; ++spin) {
            if (desc[12] & 1) { // no routed vector: poll dd
                break;
            }
        }
    }
    u32 dd = desc[12] & 1;
    u32 tdh = REG(TDH);
    u32 icr = REG(ICR);
    REG(IMC) = ~0u;
    REG(ICR) = 0xFFFFFFFFu; // the tx causes out
    u64 rep = (u64)irq | ((u64)dd << 8) | ((u64)tdh << 16) |
              ((u64)(icr & 0xFFFF) << 24);
    hyper(3, rep, 64);

    /* rx: 64 legacy descriptors of 2048-byte buffers, rdt = 63 hands the
     * whole ring to the nic; rctl.en is bit 1 (0x8012 = en|mpe|bam) */
    for (u32 i = 0; i < NRX; ++i) {
        volatile u64 *d = (volatile u64 *)(RXRING + (u64)i * 16);
        d[0] = RXBUF + (u64)i * BUFSZ;
        d[1] = 0;
    }
    REG(RDBAL) = (u32)RXRING;
    REG(RDBAH) = (u32)(RXRING >> 32);
    REG(RDLEN) = NRX * 16;
    REG(RDH) = 0;
    REG(RDT) = NRX - 1;
    REG(RCTL) = 0x8012;                        // en (bit 1!) | mpe | bam, 2048 buffers
    REG(IMS) = (1 << 7) | (1 << 4) | (1 << 2); // rxt0 | rxdmt0 | lsc
    hyper(4, our_mac & 0xFFFFFFFF, our_mac >> 32);

    /* the client round: everything is driven by the main loop, packets
     * land via the routed vector, the clock via the monitor's nudges */
    u32 phase = P_ARP;
    u32 phase_at = jiffies;
    u32 ping_seq = 1;
    u32 arp_sent = 0;
    u32 ping_sent = 0;
    u32 tcp_started = 0;
    u32 link_reported = 0;
    u32 closed_reported = 0;
    u8 seg[1536];
    for (;;) {
        while (pkq_tail != pkq_head) {
            volatile const u8 *slot =
                (volatile const u8 *)(PKTQ + (u64)pkq_tail * BUFSZ);
            process_frame(slot, (u16)pkq_len[pkq_tail], seg);
            pkq_tail = (pkq_tail + 1) % PKTS;
        }
        if (lsc_seen && !link_reported) {
            link_reported = 1;
            hyper(5, REG(STATUS), 0);
        }
        if (phase == P_ARP) {
            if (!arp_sent) {
                arp_sent = 1;
                arp_request_send();
            }
            if (gw_ok) {
                phase = P_PING;
                phase_at = jiffies;
            } else if (jiffies - phase_at > 10) {
                arp_sent = 0; /* re-arm, the gateway may be slow */
                phase_at = jiffies;
            }
        } else if (phase == P_PING) {
            if (!ping_sent) {
                ping_sent = 1;
                ping_send((u16)ping_seq);
            }
            if (ping_replied) {
                phase = P_TCP;
                phase_at = jiffies;
            } else if (jiffies - phase_at > 10) {
                ping_sent = 0;
                phase_at = jiffies;
            }
        } else if (phase == P_TCP) {
            if (!tcp_started) {
                tcp_started = 1;
                REG(TDH) = 0;
                REG(TDT) = 0;
                for (u32 i = 0; i < NTX; ++i) {
                    volatile u8 *d = (volatile u8 *)(TXRING + (u64)i * 16);
                    d[12] = 0;
                }
                conn_start(jiffies);
            }
            tcp_engine(seg);
            if (terr || tstate == T_DONE) {
                phase = P_SERVE;
            }
        } else {
            if (!closed_reported) {
                closed_reported = 1;
                if (!established_seen) {
                    hyper(10, 0, terr == E_RST ? 1 : 2);
                }
                u64 rtx =
                    ((u64)rtx_syn << 16) | ((u64)rtx_data << 8) | rtx_fin;
                hyper(14, terr == E_NONE && tstate == T_DONE ? 1 : 0, rtx);
            }
        }
        asm volatile("hlt");
    }
}
