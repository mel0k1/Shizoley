// guest side of the driver-vm round: a real e1000 driver running in the
// guest — tx AND rx. the nic dmas the descriptor rings straight out of
// guest ram (gpa addresses, translated by the vt-d domain that mirrors
// the guest ept), and the bar reads go through the ept uc leaf (emt 0) —
// neither the rings nor the bar are cacheable by any guest pat trick.
// interrupts: the monitor routes the nic's host vector into the guest
// (guest_pi), the kernel posts it (pir on apicv, entry injection without),
// and the idt handler here drains the rx ring. the stack is small but
// real: arp replies for our address and icmp echo answers to the gateway
// (qemu's user-net reflects pings to 10.0.2.2), so a complete
// request -> reply round trip happens without any host code. the results
// travel to the monitor over vmcall: nr 2 = the status read, nr 3 = the
// tx report (irq | dd<<8 | tdh<<16 | icr<<24), nr 4 = ready with the mac,
// nr 5 = link status, nr 6 = an rx packet report, nr 7 = the arp answer,
// nr 8 = the ping echo.
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

#define OUR_IP 0x6302000AUL /* 10.0.2.99, wire order */
#define GW_IP 0x0202000AUL  /* 10.0.2.2, wire order */

static inline u64 hyper3(u64 nr, u64 a1, u64 a2) {
    u64 r;
    asm volatile("vmcall" : "=a"(r) : "a"(nr), "S"(a1), "d"(a2) : "memory");
    return r;
}

/* four-argument report: the rest of the gprs ride the exit buffer */
static inline void hyper(u64 nr, u64 a1, u64 a2) {
    hyper3(nr, a1, a2);
}

extern void nic_isr(void);
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
        "    cld\n"
        "    call rx_isr\n"
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

static u32 irq_count; /* in .bss: the flat blob stays small */
static u32 rx_cursor; /* next descriptor to consume = (rdt + 1) % nrx */
static u32 tx_slot;
static u64 our_mac;
static u64 gw_mac;

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

/* post the tx buffer at the cursor and wait for the writeback */
static void tx_send(u16 len) {
    volatile u8 *d = (volatile u8 *)(TXRING + tx_slot * 16);
    u64 gpa = TXBUF + (u64)tx_slot * BUFSZ;
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

/* answer an arp request for our address */
static void do_arp(volatile const u8 *pkt) {
    if (be16(pkt + 20) != 1) {
        return; /* not a request */
    }
    if (be32(pkt + 38) != OUR_IP) {
        return; /* someone else's address */
    }
    volatile u8 *r = (volatile u8 *)(TXBUF + (u64)tx_slot * BUFSZ);
    for (int i = 0; i < 42; ++i) {
        r[i] = 0;
    }
    for (int i = 0; i < 6; ++i) {
        r[i] = pkt[6 + i];    /* dst = sender mac */
        r[6 + i] = ((volatile const u8 *)&our_mac)[i];
        r[22 + i] = ((volatile const u8 *)&our_mac)[i]; /* sha */
        r[32 + i] = pkt[22 + i]; /* tha = sender mac */
    }
    put16(r + 12, 0x0806);
    put16(r + 14, 1);
    put16(r + 16, 0x0800);
    r[18] = 6;
    r[19] = 4;
    put16(r + 20, 2); /* reply */
    put32(r + 28, OUR_IP);
    put32(r + 38, be32(pkt + 28)); /* tpa = sender ip */
    gw_mac = 0;
    for (int i = 0; i < 6; ++i) {
        ((u8 *)&gw_mac)[i] = pkt[22 + i];
    }
    tx_send(42);
    hyper(7, gw_mac & 0xFFFFFFFF, gw_mac >> 32);
}

/* answer an icmp echo request aimed at us (the gateway's ping) */
static void do_icmp(volatile const u8 *pkt, u16 len) {
    u32 ihl = (u32)(pkt[14] & 0xF) * 4;
    if (pkt[23] != 1 || len < 14 + ihl + 8) {
        return;
    }
    volatile const u8 *ic = pkt + 14 + ihl;
    if (ic[0] != 8) {
        return; /* not an echo request */
    }
    volatile u8 *r = (volatile u8 *)(TXBUF + (u64)tx_slot * BUFSZ);
    for (int i = 0; i < len; ++i) {
        r[i] = pkt[i];
    }
    for (int i = 0; i < 6; ++i) {
        r[i] = pkt[6 + i]; /* dst = sender mac */
        r[6 + i] = ((volatile const u8 *)&our_mac)[i];
    }
    put16(r + 12, 0x0800);
    r[14] = 0x45;
    put16(r + 16, (u16)(len - 14));
    put16(r + 18, be16(pkt + 18));
    r[20] = 0;
    r[22] = 0;
    r[20] = 0; /* flags/frag clear */
    r[21] = 0;
    r[22] = 64; /* ttl */
    r[23] = 1;  /* icmp */
    put16(r + 24, 0);
    put32(r + 26, OUR_IP);        /* src = us */
    put32(r + 30, be32(pkt + 26)); /* dst = original sender */
    put16(r + 24, cksum(r + 14, 20));
    r[14 + ihl] = 0; /* type = echo reply */
    put16(r + 14 + ihl + 2, 0);
    put16(r + 14 + ihl + 2, cksum(r + 14 + ihl, (u32)(len - 14 - ihl)));
    tx_send(len);
    hyper(8, be16(ic + 6), 0);
}

static void handle_pkt(volatile u8 *pkt, u16 len) {
    u16 et = be16(pkt + 12);
    u32 src = et == 0x0806 ? be32(pkt + 28) : be32(pkt + 26);
    hyper(6, (u64)len, ((u64)et << 32) | src);
    if (et == 0x0806) {
        do_arp(pkt);
    } else if (et == 0x0800) {
        do_icmp(pkt, len);
    }
}

/* the idt body: deassert the level line first (icr write-back), then
 * drain everything the nic filled */
void rx_isr(void) {
    u32 icr = REG(ICR);
    REG(ICR) = icr;
    irq_count++;
    if (icr & 4) {
        hyper(5, REG(STATUS), 0); /* link status change */
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
        if (st & 2) { /* eop: single-buffer packets */
            volatile u8 *pkt = (volatile u8 *)(RXBUF + (u64)rx_cursor * BUFSZ);
            if (len >= 34 && len <= BUFSZ) {
                handle_pkt(pkt, len);
            }
        }
        d[12] = 0; /* hand the descriptor back */
        REG(RDT) = rx_cursor;
        rx_cursor = (rx_cursor + 1) % NRX;
    }
}

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
        idt_set(NUDGE, (u64)nic_isr); /* the monitor's wake vector */
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
    REG(RCTL) = 0x8012;           // en (bit 1!) | mpe | bam, 2048 buffers
    REG(IMS) = (1 << 7) | (1 << 4) | (1 << 2); // rxt0 | rxdmt0 | lsc
    hyper(4, our_mac & 0xFFFFFFFF, our_mac >> 32);
    for (;;) {
        asm volatile("hlt");
    }
}
