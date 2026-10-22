// consoled — the keyboard/console as a ring-3 process: it registers its own
// event queue as the kernel's sink (eq_listen, LISTEN capability) and then
// renders whatever the kernel posts into it: key presses as [sc, ascii]
// events and the boot ping that proves the pipe works.
#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace {

// 8 chunks x 256 bytes is plenty for key events
static unsigned char eqmem[48 + 8 * 256] __attribute__((aligned(8)));

void on_event(const EqEvent& ev) {
    if (ev.opcode == EQ_OP_BOOT) {
        println("[console] up: kernel sink attached");
        return;
    }
    if (ev.opcode == EQ_OP_KEY && ev.len >= 2) {
        u8 sc = ev.payload[0];
        u8 ch = ev.payload[1];
        if (ch >= 32 && ch < 127) {
            println("[console] key '", static_cast<char>(ch), "' (sc ",
                    static_cast<u64>(sc), ")");
        } else {
            println("[console] key sc ", static_cast<u64>(sc));
        }
    }
}

}

namespace shz {

int main() {
    println("[consoled] pid ", static_cast<u64>(getpid()),
            ": console as a process");
    eqmem[4] = 0x00; // chunk_size = 256 (LE u32)
    eqmem[5] = 0x01;
    eqmem[8] = 8;    // nchunks
    long q = eq_setup(eqmem, sizeof(eqmem));
    if (q < 0) {
        println("[consoled] eq_setup failed");
        return 1;
    }
    if (eq_listen(q) != 0) {
        println("[consoled] listen denied (no LISTEN right?)");
        return 1;
    }
    println("[consoled] queue ", static_cast<u64>(q), " is the kernel sink");
    for (;;) {
        eq_drive(q, eqmem, on_event);
        u32 ecode = 0;
        if (eq_errors(eqmem, &ecode) > 0) {
            println("[console] eq error channel: code ", static_cast<u64>(ecode));
        }
        if (eq_wait(q) < 0) {
            // queue gone or spurious wake: pause, then retry
            for (int i = 0; i < 1000; ++i) {
                asm volatile("pause");
            }
        }
    }
}

}
