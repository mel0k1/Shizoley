// gdemo — the two-way VcpuExit smoke test in ring 3. GUEST_CREATE builds
// vmcs + EPT + ram window, then GUEST_MAP grows the guest by one page and
// the stage-2 string lives inside it. the demo round trip:
//   1. guest prints "VEDA VMX OK" into its "VGA" through the EPT, vmcall
//   2. host loads gprs (rax=0x1CEB) and re-enters: guest branches on it and
//      prints the string from the GUEST_MAPPED page (gprs + guest_map)
//   3. guest vmcalls again; host injects extint 0x20 whose real-mode IVT
//      handler writes 'I' to 0xB8100 and irets; guest halts
// on machines without VMX (or without nested virt, e.g. CI) the kernel
// refuses GUEST_CREATE and we report that gracefully.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace {

// 16-bit blob, assembled by hand:
//   xor di,di; mov ax,0xB800; mov es,ax; mov si,0x40;
// .l: lodsb; or al,al; jz .b; mov ah,0x0F; stosw; jmp .l
// .b: mov ax,0xBEEF; vmcall; cmp ax,0x1CEB; jnz .bad;
//     mov ax,0x1000; mov fs,ax; mov si,0xF0000   ; string in the mapped page
// .bad: xor di,di;
// .l2: lodsb; or al,al; jz .c; mov ah,0x2A; stosw; jmp .l2
// .c: vmcall; hlt; jmp $
//   0x40: "VEDA VMX OK"   0x50: "GPRS BAD"
const unsigned char guest_code[] = {
    0x31, 0xFF,                   // xor di, di
    0xB8, 0x00, 0xB8,             // mov ax, 0xB800
    0x8E, 0xC0,                   // mov es, ax
    0xBE, 0x40, 0x00,             // mov si, 0x40
    0xAC,                         // lodsb
    0x08, 0xC0,                   // or al, al
    0x74, 0x05,                   // jz .b
    0xB4, 0x0F,                   // mov ah, 0x0F
    0xAB,                         // stosw
    0xEB, 0xF6,                   // jmp .l
    0xB8, 0xEF, 0xBE,             // .b: mov ax, 0xBEEF
    0x0F, 0x01, 0xC1,             // vmcall
    0x3D, 0xEB, 0x1C,             // cmp ax, 0x1CEB
    0x75, 0x08,                   // jnz .bad
    0xB8, 0x00, 0x10,             // mov ax, 0x1000
    0x8E, 0xE0,                   // mov fs, ax
    0xBE, 0x00, 0xF0,             // mov si, 0xF0000
    0x31, 0xFF,                   // .bad: xor di, di
    0xAC,                         // .l2: lodsb
    0x08, 0xC0,                   // or al, al
    0x74, 0x05,                   // jz .c
    0xB4, 0x2A,                   // mov ah, 0x2A
    0xAB,                         // stosw
    0xEB, 0xFB,                   // jmp .l2
    0x0F, 0x01, 0xC1,             // .c: vmcall
    0xF4,                         // hlt
    0xEB, 0xFE,                   // jmp $
    // pad to 0x40, then the stage-1 string
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90,
    'V', 'E', 'D', 'A', ' ', 'V', 'M', 'X', ' ', 'O', 'K', 0,
};
static_assert(sizeof(guest_code) == 0x4C, "blob layout");

// real-mode int 0x20 handler at phys 0x200, wired through the IVT:
//   es:mov byte [0x8100],'I'; es:mov byte [0x8101],0x2A; iret
const unsigned char int20_handler[] = {
    0x26, 0xC6, 0x06, 0x00, 0x81, 0x49, // es: mov byte [0x8100], 'I'
    0x26, 0xC6, 0x06, 0x01, 0x81, 0x2A, // es: mov byte [0x8101], 0x2A
    0xCF,                               // iret
};

// the string the guest reads from the GUEST_MAPPED page (guest phys 0x100000)
const char mapped_str[] = "GPRS+MAP OK";

}

namespace shz {

int main() {
    long g = guest_create(256); // 1 MiB guest ram, covers 0xB8000
    if (g < 0) {
        println("[gdemo] vmx unavailable (guest_create refused)");
        return 0;
    }
    println("[gdemo] guest ", static_cast<u64>(g), " created (vmcs+ept+1MiB)");

    volatile u8* win = reinterpret_cast<volatile u8*>(GUEST_WINDOW);
    for (u64 i = 0; i < sizeof(guest_code); ++i) {
        win[i] = guest_code[i];
    }

    // GUEST_MAP: +1 page at guest phys 0x100000; the stage-2 string lives
    // there, so the guest only finds it if the runtime mapping worked
    long total = guest_map(g, 1);
    if (total != 257) {
        println("[gdemo] guest_map failed (", static_cast<u64>(total), ")");
        guest_destroy(g);
        return 1;
    }
    println("[gdemo] guest_map: guest ram now ", static_cast<u64>(total), " pages");
    volatile u8* mapped = win + 0x100000;
    for (u64 i = 0; i < sizeof(mapped_str); ++i) {
        mapped[i] = static_cast<u8>(mapped_str[i]);
    }

    // IVT vector 0x20 -> 0x0000:0x0200, handler right there
    win[0x80] = 0x00;
    win[0x81] = 0x02;
    win[0x82] = 0x00;
    win[0x83] = 0x00;
    for (u64 i = 0; i < sizeof(int20_handler); ++i) {
        win[0x200 + i] = int20_handler[i];
    }

    VcpuExit ve{};
    long reason = -1;
    int stage = 0;
    for (int i = 0; i < 1000; ++i) {
        reason = vcpu_run(g, &ve);
        if (reason < 0) {
            println("[gdemo] vcpu_run failed");
            guest_destroy(g);
            return 1;
        }
        if (reason == static_cast<long>(VmxExit::ext_int) ||
            reason == static_cast<long>(VmxExit::preempt_timer)) {
            continue; // host took the cpu back (tick or p3 timer): re-enter
        }
        if (reason == static_cast<long>(VmxExit::vmcall) && stage == 0) {
            // host -> guest: gpr override, the guest branches on rax
            stage = 1;
            ve.gprs[0] = 0x1CEB; // rax
            ve.flags = EXIT_IN_GPRS;
            continue;
        }
        if (reason == static_cast<long>(VmxExit::vmcall) && stage == 1) {
            // host -> guest: deliver extint 0x20 on the next entry
            stage = 2;
            ve.flags = EXIT_INJECT;
            ve.inject = vexit_extint(0x20);
            ve.err = 0;
            ve.instr_len = 0;
            continue;
        }
        break; // hlt (or anything unexpected): done
    }
    println("[gdemo] exit reason ", static_cast<u64>(reason), " rip ", ve.out[1]);

    // read what the guest wrote through the EPT windows
    char text[16];
    u64 n = 0;
    for (u64 i = 0; i < 15; ++i) {
        u8 c = win[0xB8000 + i * 2];
        if (c == 0) {
            break;
        }
        text[n++] = static_cast<char>(c);
    }
    text[n] = '\0';
    println("[gdemo] guest says: '", text, "'");

    if (win[0xB8100] == 'I') {
        println("[gdemo] inject: int 0x20 handler ran (two-way exit ok)");
    } else {
        println("[gdemo] inject: handler did not run");
    }

    if (guest_destroy(g) == 0) {
        println("[gdemo] guest destroyed, frames back in the pool");
    }
    return 0;
}

}
