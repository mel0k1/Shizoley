// guest side of the driver-vm round: a real e1000 tx driver running in
// the guest. the nic dmas the descriptor ring straight out of guest ram
// (gpa addresses, translated by the vt-d domain that mirrors the guest
// ept), and the bar reads go through the ept uc leaf (emt 0) — neither
// the ring nor the bar are cacheable by any guest pat trick. tx
// completion arrives as an interrupt: the monitor routes the nic's host
// vector into the guest (guest_pi), the kernel posts it (pir on apicv,
// entry injection without) and the idt handler here counts it — no dd
// polling. the results travel to the monitor over vmcall: nr 2 = the
// status read, nr 3 = the tx report (irq | dd<<8 | tdh<<16 | icr<<24,
// len in rdx).
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
#define TCTL 0x0400
#define TDBAL 0x3800
#define TDBAH 0x3804
#define TDLEN 0x3808
#define TDH 0x3810
#define TDT 0x3818

#define TXRING 0xA000UL
#define TXBUF 0xB000UL
#define IDT 0x7000UL
#define VEC 0xC008UL

static inline u64 hyper(u64 nr, u64 a1, u64 a2) {
    u64 r;
    asm volatile("vmcall" : "=a"(r) : "a"(nr), "S"(a1), "d"(a2) : "memory");
    return r;
}

// the irq counter lives in .text next to the stub: a .data symbol would
// pull the default linker layout in and blow the blob up to megabytes
extern u32 irq_count;

extern void nic_isr_stub(void);
__asm__(".text\n"
        "nic_isr_stub:\n"
        "    incl irq_count(%rip)\n"
        "    iretq\n"
        "irq_count:\n"
        "    .long 0\n");

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

void guest_main(void) {
    u32 status = REG(STATUS);
    hyper(2, status, 8);
    u64 vec = *(volatile u64 *)VEC;

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

    REG(IMC) = ~0u;
    REG(ICR) = 0xFFFFFFFFu; // stale causes out
    u32 irq = 0;
    if (vec) {
        idt_set(vec, (u64)nic_isr_stub);
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
    REG(TCTL) = 0;
    u64 rep = (u64)irq | ((u64)dd << 8) | ((u64)tdh << 16) |
              ((u64)(icr & 0xFFFF) << 24);
    hyper(3, rep, 64);
    for (;;) {
        asm volatile("hlt");
    }
}
