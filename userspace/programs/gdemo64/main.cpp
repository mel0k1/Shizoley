// gdemo64 — the long-mode guest smoke test in ring 3 (the "guest farther"
// roadmap step). guest_mode(1) rewrites the vmcs guest state for ia-32e
// entry; the monitor preloads page tables, a gdt and 64-bit code into the
// guest ram through its window and points rip/rsp/cr3 at them via the
// vcpu_run control half. the round trip:
//   1. guest (cpl 0, long mode, identity 2 MiB pages) vmcalls with
//      rax=1, rsi=msg gpa, rdx=len — the host reads the string straight
//      out of the guest ram window and prints it
//   2. host -> guest channel: the host loads rax=0xBEEF via EXIT_IN_GPRS;
//      the guest branches on it (bad -> ud2, an #UD exit) and halts
// on machines without VMX (or without nested virt, e.g. CI) the kernel
// refuses GUEST_CREATE and we report that gracefully.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace {

// 64-bit blob at guest phys 0x10000, assembled by hand:
//   mov eax,1; mov esi,0x10800; mov edx,13; vmcall;
//   cmp eax,0xBEEF; jnz .bad; hlt; .bad: ud2
const unsigned char guest64[] = {
    0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1        (hypercall: console)
    0xBE, 0x00, 0x08, 0x01, 0x00, // mov esi, 0x10800  (msg gpa)
    0xBA, 0x0D, 0x00, 0x00, 0x00, // mov edx, 13
    0x0F, 0x01, 0xC1,             // vmcall
    0x3D, 0xEF, 0xBE, 0x00, 0x00, // cmp eax, 0xBEEF
    0x75, 0x01,                   // jnz .bad
    0xF4,                         // hlt
    0x0F, 0x0B,                   // .bad: ud2
};
static_assert(sizeof(guest64) == 0x1C, "blob layout");

const char guest_msg[] = "HELLO FROM L2";
static_assert(sizeof(guest_msg) - 1 == 13, "msg length");

// guest phys layout (fixed convention, see kernel vmx.rs apply_long_mode):
//   0x2000 pml4  0x3000 pdpt  0x4000 pd (16 x 2 MiB leaves = 32 MiB identity)
//   0x5000 gdt (null + cs + ds)   0x6000 idt area   0x10000 code   0x10800 msg
constexpr u64 GPA_PML4 = 0x2000;
constexpr u64 GPA_PDPT = 0x3000;
constexpr u64 GPA_PD = 0x4000;
constexpr u64 GPA_GDT = 0x5000;
constexpr u64 GPA_CODE = 0x10000;
constexpr u64 GPA_MSG = 0x10800;
constexpr u64 ENTRY_RIP = 0x10000;
constexpr u64 ENTRY_RSP = 0x1F000;

} // namespace

namespace shz {

int main() {
    long g = guest_create(32); // 128 KiB guest ram, covers the layout above
    if (g < 0) {
        println("[gdemo64] vmx unavailable (guest_create refused)");
        return 0;
    }
    println("[gdemo64] guest ", static_cast<u64>(g), " created (128 KiB)");

    volatile u8* win = reinterpret_cast<volatile u8*>(GUEST_WINDOW);
    volatile u64* win64 = reinterpret_cast<volatile u64*>(GUEST_WINDOW);

    // long-mode identity map: 32 MiB through 2 MiB leaves (P|W|PS)
    win64[GPA_PML4 / 8] = GPA_PDPT | 0x03; // pml4[0] -> pdpt
    win64[GPA_PDPT / 8] = GPA_PD | 0x03;   // pdpt[0] -> pd
    for (int i = 0; i < 16; ++i) {
        win64[GPA_PD / 8 + i] = (static_cast<u64>(i) << 21) | 0x87;
    }
    // gdt: null, 64-bit code (l=1 g=1 rx), flat data (g=1 rw)
    win64[GPA_GDT / 8 + 0] = 0;
    win64[GPA_GDT / 8 + 1] = 0x00AF9A000000FFFFull;
    win64[GPA_GDT / 8 + 2] = 0x00CF92000000FFFFull;
    // code + message
    for (u64 i = 0; i < sizeof(guest64); ++i) {
        win[GPA_CODE + i] = guest64[i];
    }
    for (u64 i = 0; i < sizeof(guest_msg); ++i) {
        win[GPA_MSG + i] = static_cast<u8>(guest_msg[i]);
    }

    if (guest_mode(g, 1) != 0) {
        println("[gdemo64] guest_mode(long) refused");
        guest_destroy(g);
        return 1;
    }

    VcpuExit ve{};
    ve.flags = EXIT_IN_ENTRY;
    ve.entry_rip = ENTRY_RIP;
    ve.entry_rsp = ENTRY_RSP;
    ve.entry_cr3 = GPA_PML4;

    long reason = -1;
    bool console_ok = false;
    bool proof_ok = false;
    for (int i = 0; i < 1000; ++i) {
        reason = vcpu_run(g, &ve);
        if (reason < 0) {
            println("[gdemo64] vcpu_run failed");
            guest_destroy(g);
            return 1;
        }
        if (reason == static_cast<long>(VmxExit::ext_int) ||
            reason == static_cast<long>(VmxExit::preempt_timer)) {
            continue; // host took the cpu back: re-enter
        }
        if (reason == static_cast<long>(VmxExit::vmcall) && !console_ok) {
            // out[4..20] mirror the gprs: rax=0 rdx=3 rsi=4
            u64 rax = ve.out[4];
            u64 rdx = ve.out[7];
            u64 rsi = ve.out[8];
            if (rax != 1 || rdx > 64 || rsi + rdx > 0x20000) {
                println("[gdemo64] bogus hypercall rax ", rax);
                break;
            }
            char text[65];
            for (u64 k = 0; k < rdx; ++k) {
                text[k] = static_cast<char>(win[rsi + k]);
            }
            text[rdx] = '\0';
            println("[gdemo64] hypercall: '", text, "'");
            console_ok = true;
            // host -> guest: hand the proof value back through rax
            ve.flags = EXIT_IN_GPRS;
            ve.gprs[0] = 0xBEEF;
            continue;
        }
        if (reason == static_cast<long>(VmxExit::hlt)) {
            println("[gdemo64] guest halted");
            proof_ok = console_ok; // reached hlt only if the branch was taken
            break;
        }
        if (reason == static_cast<long>(VmxExit::ud)) {
            println("[gdemo64] guest #UD: rax proof failed");
            break;
        }
        println("[gdemo64] unexpected exit ", static_cast<u64>(reason));
        break;
    }

    if (console_ok && proof_ok) {
        println("[gdemo64] long-mode round trip ok (hypercall + gpr channel)");
    }

    if (guest_destroy(g) == 0) {
        println("[gdemo64] guest destroyed, frames back in the pool");
    }
    return console_ok && proof_ok ? 0 : 1;
}

}
