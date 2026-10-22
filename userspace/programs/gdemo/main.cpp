// gdemo — the Veda-approach smoke test in ring 3: GUEST_CREATE builds a
// vmcs + EPT + ram window, we drop a 16-bit real-mode blob at guest phys 0,
// VCPU_RUN enters it. the guest writes text into its own "VGA" memory at
// 0xB8000 through the EPT and stops itself with a vmcall; we then read the
// text back through the window — a full hypervisor round trip.
//
// on machines without VMX (or without nested virt, e.g. CI) the kernel
// refuses GUEST_CREATE and we report that gracefully.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace {

// 16-bit blob, assembled by hand:
//   xor di,di; mov ax,0xB800; mov es,ax; mov si,0x20;
// .l: lodsb; or al,al; jz .done; mov ah,0x0F; stosw; jmp .l
//   .done: vmcall; hlt; jmp $
//   0x20: "VEDA VMX OK"
const unsigned char guest_code[] = {
    0x31, 0xFF,                   // xor di, di
    0xB8, 0x00, 0xB8,             // mov ax, 0xB800
    0x8E, 0xC0,                   // mov es, ax
    0xBE, 0x20, 0x00,             // mov si, 0x20
    0xAC,                         // lodsb
    0x08, 0xC0,                   // or al, al
    0x74, 0x05,                   // jz .done
    0xB4, 0x0F,                   // mov ah, 0x0F
    0xAB,                         // stosw
    0xEB, 0xF6,                   // jmp .l
    0x0F, 0x01, 0xC1,             // vmcall
    0xF4,                         // hlt
    0xEB, 0xFE,                   // jmp $
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // pad to 0x20
    'V', 'E', 'D', 'A', ' ', 'V', 'M', 'X', ' ', 'O', 'K', 0,
};

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

    u64 out[20];
    long reason = vcpu_run(g, out);
    if (reason < 0) {
        println("[gdemo] vcpu_run failed");
        guest_destroy(g);
        return 1;
    }
    println("[gdemo] exit reason ", static_cast<u64>(reason), " rip ",
            out[1], " qual ", out[2]);

    // read what the guest wrote through the EPT window
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

    if (guest_destroy(g) == 0) {
        println("[gdemo] guest destroyed, frames back in the pool");
    }
    return 0;
}

}
