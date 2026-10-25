// guest side of the driver-vm round: a real e1000 tx driver running in
// the guest. the nic dmas the descriptor ring straight out of guest ram
// (gpa addresses, translated by the vt-d domain that mirrors the guest
// ept), and the bar reads go through the ept uc leaf (emt 0) — neither
// the ring nor the bar are cacheable by any guest pat trick. the results
// travel to the monitor over vmcall: nr 2 = the status read, nr 3 = the
// tx report (dd | tdh<<8 | icr<<16, len in rdx).
typedef unsigned long u64;
typedef unsigned int u32;
typedef unsigned short u16;
typedef unsigned char u8;

#define BAR 0xFEBC0000UL
#define REG(off) (*(volatile u32 *)(BAR + (off)))

#define CTRL 0x0000
#define STATUS 0x0008
#define ICR 0x00C0
#define TCTL 0x0400
#define TDBAL 0x3800
#define TDBAH 0x3804
#define TDLEN 0x3808
#define TDH 0x3810
#define TDT 0x3818

#define TXRING 0xA000UL
#define TXBUF 0xB000UL

static inline u64 hyper(u64 nr, u64 a1, u64 a2) {
    u64 r;
    asm volatile("vmcall" : "=a"(r) : "a"(nr), "S"(a1), "d"(a2) : "memory");
    return r;
}

void guest_main(void) {
    u32 status = REG(STATUS);
    hyper(2, status, 8);

    REG(CTRL) = 0x40;         // slu
    REG(TDBAL) = (u32)TXRING; // ring in guest ram: the nic's dma reaches it
                              // only through the guest's vt-d domain
    REG(TDBAH) = 0;
    REG(TDLEN) = 4 * 16;
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

    REG(TDT) = 1; // kick
    u32 dd = 0;
    for (long spin = 0; spin < 8000000; ++spin) {
        dd = desc[12] & 1;
        if (dd) {
            break;
        }
    }
    u32 tdh = REG(TDH);
    u32 icr = REG(ICR);
    REG(TCTL) = 0;
    u64 rep = (u64)dd | ((u64)tdh << 8) | ((u64)(icr & 0xFFFF) << 16);
    hyper(3, rep, 64);
    for (;;) {
        asm volatile("hlt");
    }
}
