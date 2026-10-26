// devd — the driver-vm round: the nic goes into the guest's iommu domain,
// its bar0 is mapped into the guest ept identity as uc (emt 0, the guest
// pat can never make it cacheable) and the guest runs a REAL e1000 driver
// (guest.c): tx and rx rings in guest ram (gpa addresses — the nic's dma
// is translated by the vt-d domain that mirrors the guest ept), a tx
// completion by interrupt (the nic's host intx vector routed in via
// guest_pi, posted as pir on apicv or entry-injected without), then the
// rx side: a tiny network stack that answers arp, pings the gateway back
// (icmp echo over qemu's user-net) and echoes udp datagrams that arrive
// for its service port 4242, all interrupt-driven — the monitor only
// wakes the idle guest with a nudge vector. results come over vmcall:
// 2 = the status read, 3 = the tx report (irq | dd<<8 | tdh<<16 |
// icr<<24), 4 = ready with the mac, 5 = link status, 6 = an rx packet
// report, 7 = the arp answer, 8 = the ping echo, 9 = the udp echo. after
// the ping lands the monitor holds the guest for an extra window so a
// udp datagram sent to the hostfwd port can complete the round. under
// tcg (no vmx) it reports and exits.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

#include <guest_blob.h>

using namespace shz;

namespace {

// guest phys layout (224 kib of guest ram: 16 base pages + 40 grown):
//   0x1000 pml4  0x2000 pdpt  0x3000 pd (identity 2 MiB leaves for ram)
//   0x4000 pdpt for pml4[3]   0x5000 pd for the bar's 1 gib region
//   0x6000+ pt frames for the bar window   0x7000 guest idt  0x8000 gdt
//   0x9000 code   0xC008 nic host vector handed to the guest
//   0x10000 rx ring (64x16)  0x11000 rx buffers (64x2048)
//   0x32000 tx ring (4x16)   0x33000 tx payload
constexpr u64 GPA_PML4 = 0x1000;
constexpr u64 GPA_PDPT = 0x2000;
constexpr u64 GPA_PD = 0x3000;
constexpr u64 GPA_PDPT3 = 0x4000;
constexpr u64 GPA_PD3 = 0x5000;
constexpr u64 GPA_PT0 = 0x6000;
constexpr u64 GPA_GDT = 0x8000;
constexpr u64 ENTRY_RIP = 0x9000; // guest.c is linked with -Ttext=0x9000
constexpr u64 ENTRY_RSP = 0xF000;
constexpr u64 GPA_VEC = 0xC008;   // nic host vector handed to the guest
constexpr u64 GROW_PAGES = 40;    // guest ram: 16 + 40 = 224 kib

constexpr u64 SID_E1000 = 0x0010; // 00:02.0 on the reference qemu command line
constexpr u64 NUDGE_VECTOR = 0x22; // the monitor's wake vector (see guest.c)

void puthex(u64 v) {
    static const char digits[] = "0123456789abcdef";
    char out[19];
    out[0] = '0';
    out[1] = 'x';
    for (int i = 0; i < 16; ++i) {
        out[2 + i] = digits[(v >> (60 - i * 4)) & 0xF];
    }
    out[18] = '\0';
    print(out);
}

} // namespace

namespace shz {

int main() {
    long g = guest_create(16);
    if (g < 0) {
        println("[devd] vmx unavailable (guest_create refused)");
        return 0;
    }
    if (guest_map(g, GROW_PAGES) < 0) {
        println("[devd] guest_map refused");
        guest_destroy(g);
        return 1;
    }
    if (guest_device_attach(g, SID_E1000) != 0) {
        println("[devd] device 00:02.0 attach refused");
        guest_destroy(g);
        return 1;
    }
    long size = guest_mmio(g, SID_E1000, 0);
    if (size <= 0) {
        println("[devd] bar0 map refused (not a memory bar or device detached)");
        guest_device_detach(g, SID_E1000);
        guest_destroy(g);
        return 1;
    }
    u64 pages = (static_cast<u64>(size) + 0xFFF) / 0x1000;
    println("[devd] bar0 mapped: ", static_cast<u64>(size), " bytes (",
            pages, " pages, uc via emt)");

    volatile u8* win = reinterpret_cast<volatile u8*>(GUEST_WINDOW);
    volatile u64* win64 = reinterpret_cast<volatile u64*>(GUEST_WINDOW);

    // the nic's host intx vector (discovered by the kernel smoke) is what
    // the guest waits on: route it and hand the number over in guest ram
    long vec = nic_vec();
    if (vec > 0 && guest_pi(g, static_cast<u64>(vec)) == 0) {
        win64[GPA_VEC / 8] = static_cast<u64>(vec);
        println("[devd] nic irq 0x", static_cast<u64>(vec),
                " -> guest (routed)");
    } else {
        vec = 0;
        println("[devd] no nic irq vector — guest will poll dd");
    }

    // identity ram: 4 mib through 2 mib leaves
    win64[GPA_PML4 / 8] = GPA_PDPT | 0x03;
    win64[GPA_PML4 / 8 + 3] = GPA_PDPT3 | 0x03;
    win64[GPA_PDPT / 8] = GPA_PD | 0x03;
    win64[GPA_PD / 8] = 0ull | 0x87;
    win64[GPA_PD / 8 + 1] = 0x200000ull | 0x87;
    // bar window: pdpt3 for pml4[3], pd3 for the bar's 1 gib region, one
    // pt frame per 2 mib region the bar touches; 4k ptes with pcd (the
    // ept emt=uc makes it stick regardless)
    win64[GPA_PDPT3 / 8 + ((0xFEBC0000ull >> 30) & 511)] = GPA_PD3 | 0x03;
    u64 pt_gpa = GPA_PT0;
    u64 cur_pd = 0xFFFF;
    for (u64 p = 0; p < pages; ++p) {
        u64 a = 0xFEBC0000ull + p * 0x1000;
        u64 pd_i = (a >> 21) & 511;
        if (pd_i != cur_pd) {
            win64[GPA_PD3 / 8 + pd_i] = pt_gpa | 0x03;
            cur_pd = pd_i;
            pt_gpa += 0x1000;
        }
        win64[(pt_gpa - 0x1000) / 8 + ((a >> 12) & 511)] = a | 0x13;
    }
    // gdt: null, 64-bit code (l=1 g=1 rx), flat data (g=1 rw)
    win64[GPA_GDT / 8] = 0;
    win64[GPA_GDT / 8 + 1] = 0x00AF9A000000FFFFull;
    win64[GPA_GDT / 8 + 2] = 0x00CF92000000FFFFull;
    for (u64 i = 0; i < sizeof(guest_blob); ++i) {
        win[ENTRY_RIP + i] = guest_blob[i];
    }

    if (guest_mode(g, 1) != 0) {
        println("[devd] guest_mode(long) refused");
        guest_device_detach(g, SID_E1000);
        guest_destroy(g);
        return 1;
    }

    VcpuExit ve{};
    ve.flags = EXIT_IN_ENTRY;
    ve.entry_rip = ENTRY_RIP;
    ve.entry_rsp = ENTRY_RSP;
    ve.entry_cr3 = GPA_PML4;

    long reason = -1;
    bool ok = false;
    bool tx_ok = false;
    bool ready = false;
    bool arp_ok = false;
    bool ping_ok = false;
    bool udp_ok = false;
    bool udp_window = false;
    u64 posted = 0;
    u64 start = ticks();
    u64 deadline = start + 600; // 6 s budget at 100 hz
    for (;;) {
        reason = vcpu_run(g, &ve);
        if (reason < 0) {
            // vm-entry refuses events while the guest sits in an sti
            // shadow: drop the injection, the next nudge picks up
            if (ve.flags & EXIT_INJECT) {
                ve.flags = 0;
                continue;
            }
            println("[devd] vcpu_run failed");
            break;
        }
        if (ticks() > deadline) {
            println("[devd] budget exhausted");
            break;
        }
        if (reason == static_cast<long>(VmxExit::ext_int) ||
            reason == static_cast<long>(VmxExit::preempt_timer)) {
            if (reason == static_cast<long>(VmxExit::ext_int) && ve.out[3] != 0) {
                posted++;
            }
            ve.flags = 0;
            continue;
        }
        if (reason == static_cast<long>(VmxExit::vmcall)) {
            u64 rax = ve.out[4];
            u64 rdx = ve.out[7];
            u64 rsi = ve.out[8];
            ve.flags = 0;
            if (rax == 2) {
                print("[devd] bar0 status = ");
                puthex(rsi);
                println("");
                ok = rsi != 0; // real mmio: device id, link and duplex bits
            } else if (rax == 3) {
                u64 irq = rsi & 0xFF;
                u64 dd = (rsi >> 8) & 1;
                u64 tdh = (rsi >> 16) & 0xFF;
                u64 icr = (rsi >> 24) & 0xFFFF;
                println("[devd] tx: irq ", irq, " dd ", dd, " tdh ", tdh,
                        " icr ", icr);
                tx_ok = irq != 0 && dd != 0 && tdh == 1;
                println("[devd] driver vm: nic dma'd the guest ring ",
                        tx_ok ? "(dd set, tdh advanced)" : "(no completion)");
                if (irq != 0) {
                    println("[devd] completion by interrupt: ", posted,
                            " exit(s) posted");
                }
            } else if (rax == 4) {
                ready = true;
                print("[devd] guest live: mac ");
                puthex(rsi);
                print(" ");
                puthex(rdx & 0xFFFF);
                println("");
            } else if (rax == 5) {
                print("[devd] link status = ");
                puthex(rsi);
                println("");
            } else if (rax == 6) {
                print("[devd] rx: len=");
                print(rsi);
                print(" type=");
                puthex(rdx >> 32);
                print(" peer=");
                puthex(rdx & 0xFFFFFFFF);
                println("");
            } else if (rax == 7) {
                arp_ok = true;
                println("[devd] arp answered for our address");
            } else if (rax == 8) {
                ping_ok = true;
                println("[devd] ping ok: echo reply seq ", rsi & 0xFFFF);
            } else if (rax == 9) {
                udp_ok = true;
                u32 ip = static_cast<u32>(rdx >> 32);
                println("[devd] udp echo: ", rsi, " bytes from ",
                        static_cast<u64>((ip >> 24) & 0xFF), ".",
                        static_cast<u64>((ip >> 16) & 0xFF), ".",
                        static_cast<u64>((ip >> 8) & 0xFF), ".",
                        static_cast<u64>(ip & 0xFF), ":",
                        static_cast<u64>(rdx & 0xFFFF));
            } else {
                println("[devd] bogus hypercall rax ", rax);
                break;
            }
            if (ping_ok && !udp_window) {
                // keep the guest alive a little longer so a udp datagram
                // sent to the hostfwd port can complete the round trip
                udp_window = true;
                deadline = ticks() + 300;
            }
            if ((ping_ok && udp_ok) ||
                (udp_window && ticks() > deadline)) {
                break;
            }
            continue;
        }
        if (reason == static_cast<long>(VmxExit::ept_violation)) {
            println("[devd] ept violation (no uc map or ring outside ram?)");
            break;
        }
        if (reason == static_cast<long>(VmxExit::ept_misconfig)) {
            println("[devd] ept misconfig (bad emt or flags)");
            break;
        }
        if (reason == static_cast<long>(VmxExit::ud)) {
            println("[devd] guest #UD");
            break;
        }
        if (reason == static_cast<long>(VmxExit::hlt)) {
            // idle: wake the guest with the nudge so packets that landed
            // while it slept are drained, then pace the loop
            ve.flags = EXIT_INJECT;
            ve.inject = vexit_extint(NUDGE_VECTOR);
            tsleep(30);
            continue;
        }
        println("[devd] unexpected exit ", static_cast<u64>(reason));
        break;
    }

    if (ok) {
        println("[devd] mmio round ok: uc bar read from the guest");
    }
    if (tx_ok) {
        println("[devd] driver vm ok: tx through the guest's own ring");
    }
    if (ready && arp_ok && ping_ok) {
        println("[devd] rx path ok: irq-driven arp + icmp echo in the guest");
    } else if (ready) {
        println("[devd] rx incomplete: arp=", static_cast<u64>(arp_ok ? 1 : 0),
                " ping=", static_cast<u64>(ping_ok ? 1 : 0));
    }
    if (udp_ok) {
        println("[devd] udp ok: echo answered on port 4242");
    } else if (udp_window) {
        println("[devd] udp: no datagram in the window (nothing sent to 4242?)");
    }
    guest_device_detach(g, SID_E1000);
    guest_destroy(g);
    println("[devd] device detached, guest destroyed");
    return (ok && tx_ok && ready && arp_ok && ping_ok) ? 0 : 1;
}

} // namespace shz
