// devd — the driver-vm round trip: the nic goes into the guest's iommu
// domain, its bar0 is mapped into the guest ept identity as uc (emt 0,
// the guest pat can never make it cacheable) and a long-mode guest reads
// the e1000 STATUS register straight from mmio. a non-zero status handed
// back through rsi on a vmcall proves the whole path: ept uc leaf, guest
// 4k pte with pcd, real device registers. under tcg (no vmx) it reports
// and exits cleanly.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace {

// mov eax,0xfebc0000; mov esi,[eax]; mov eax,2; mov edx,8; vmcall; hlt; ud2
const unsigned char guest_blob[] = {
    0xB8, 0x00, 0x00, 0xBC, 0xFE, // mov eax, 0xFEBC0000
    0x8B, 0x30,                   // mov esi, [rax]    (e1000 STATUS)
    0xB8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2        (hypercall: gpr echo)
    0xBA, 0x08, 0x00, 0x00, 0x00, // mov edx, 8
    0x0F, 0x01, 0xC1,             // vmcall
    0xF4,                         // hlt
    0x0F, 0x0B,                   // ud2
};

// guest phys layout (64 kib of guest ram):
//   0x1000 pml4  0x2000 pdpt  0x3000 pd (identity 2 MiB leaves for ram)
//   0x4000 pdpt for pml4[3]   0x5000 pd for the bar's 1 gib region
//   0x6000+ pt frames for the bar window   0x8000 gdt   0x9000 code
constexpr u64 GPA_PML4 = 0x1000;
constexpr u64 GPA_PDPT = 0x2000;
constexpr u64 GPA_PD = 0x3000;
constexpr u64 GPA_PDPT3 = 0x4000;
constexpr u64 GPA_PD3 = 0x5000;
constexpr u64 GPA_PT0 = 0x6000;
constexpr u64 GPA_GDT = 0x8000;
constexpr u64 GPA_CODE = 0x9000;
constexpr u64 ENTRY_RIP = GPA_CODE;
constexpr u64 ENTRY_RSP = 0xF000;

constexpr u64 SID_E1000 = 0x0010; // 00:02.0 on the reference qemu command line

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
        win[GPA_CODE + i] = guest_blob[i];
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
    for (int i = 0; i < 1000; ++i) {
        reason = vcpu_run(g, &ve);
        if (reason < 0) {
            println("[devd] vcpu_run failed");
            break;
        }
        if (reason == static_cast<long>(VmxExit::ext_int) ||
            reason == static_cast<long>(VmxExit::preempt_timer)) {
            continue;
        }
        if (reason == static_cast<long>(VmxExit::vmcall)) {
            u64 rax = ve.out[4];
            u64 rsi = ve.out[8];
            if (rax != 2) {
                println("[devd] bogus hypercall rax ", rax);
                break;
            }
            print("[devd] bar0 status = ");
            puthex(rsi);
            println("");
            ok = rsi != 0; // real mmio: device id, link and duplex bits
            ve.flags = 0;
            continue;
        }
        if (reason == static_cast<long>(VmxExit::ept_violation)) {
            println("[devd] ept violation on the bar access (no uc map?)");
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
            break;
        }
        println("[devd] unexpected exit ", static_cast<u64>(reason));
        break;
    }

    if (ok) {
        println("[devd] mmio round ok: uc bar read from the guest");
    }
    guest_device_detach(g, SID_E1000);
    guest_destroy(g);
    println("[devd] device detached, guest destroyed");
    return ok ? 0 : 1;
}

} // namespace shz
