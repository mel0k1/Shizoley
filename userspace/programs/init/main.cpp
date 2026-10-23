#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

static unsigned char stack1[8192] __attribute__((aligned(16)));
static unsigned char stack2[8192] __attribute__((aligned(16)));

// the initrd queue memory: 4 chunks x 512 bytes
static unsigned char eqmem[48 + 4 * 512] __attribute__((aligned(8)));

// user thread entry: the kernel puts arg in rdi, so this matches the SysV ABI
extern "C" void worker(u64 arg) {
    for (u64 i = 0; i < 3; ++i) {
        println("[t", arg, "] tick ", i);
    }
    exit(0);
}

namespace {

bool g_first_event = true;
bool g_zcopy_ok = false;

void on_event(const EqEvent& ev) {
    // zero-copy gift: payload = u64 handle + u64 bytes (little endian)
    if (ev.opcode == EQ_OP_ZMSG && ev.len >= 16) {
        u64 handle = 0;
        u64 bytes = 0;
        for (int i = 7; i >= 0; --i) {
            handle = (handle << 8) | ev.payload[i];
        }
        for (int i = 15; i >= 8; --i) {
            bytes = (bytes << 8) | ev.payload[i];
        }
        long va = eq_zmap(static_cast<long>(handle));
        if (va < 0) {
            println("[eq] zcopy: zmap failed for handle ", handle);
            return;
        }
        const u8* p = reinterpret_cast<const u8*>(static_cast<u64>(va));
        bool ok = true;
        for (u64 i = 0; i < bytes; ++i) {
            if (p[i] != static_cast<u8>(i & 0xFF)) {
                ok = false;
                break;
            }
        }
        eq_zdone(static_cast<long>(handle));
        if (ok) {
            println("[eq] zcopy: ", bytes, " bytes via handle ", handle,
                    ": pattern ok");
            g_zcopy_ok = true;
        } else {
            println("[eq] zcopy: pattern MISMATCH via handle ", handle);
        }
        return;
    }
    if (g_first_event) {
        char name[8];
        u64 k = 0;
        for (u32 i = 0; i < ev.len && k < 7; ++i) {
            name[k++] = static_cast<char>(ev.payload[i]);
        }
        name[k] = '\0';
        println("[eq] first event: '", name, "' op ",
                static_cast<u64>(ev.opcode), " from ", ev.sender);
        g_first_event = false;
    }
}

void put_str(char* dst, u64& n, const char* s) {
    while (*s != '\0') {
        dst[n++] = *s++;
    }
}

void put_num(char* dst, u64& n, u64 v) {
    char tmp[20];
    int i = 0;
    if (v == 0) {
        tmp[i++] = '0';
    }
    while (v > 0) {
        tmp[i++] = static_cast<char>('0' + v % 10);
        v /= 10;
    }
    while (i > 0) {
        dst[n++] = tmp[--i];
    }
}

long request(long pid, const char* what, u64 len, char* buf, u64 max) {
    send(static_cast<u64>(pid), what, len);
    return recv(static_cast<u64>(pid), buf, max);
}

}

namespace shz {

int main() {
    println("[init] pid ", static_cast<u64>(getpid()));

    long t1 = thread_create(worker, stack1 + sizeof(stack1), 1);
    long t2 = thread_create(worker, stack2 + sizeof(stack2), 2);
    println("[init] threads: ", static_cast<u64>(t1), " ", static_cast<u64>(t2));

    long hello = spawn("hello", 5);
    println("[init] spawned hello (", static_cast<u64>(hello), ")");
    send(static_cast<u64>(hello), "ping", 5);
    char buf[128];
    long from = recv(static_cast<u64>(hello), buf, sizeof(buf));
    println("[init] ", buf, " from ", static_cast<u64>(from));

    // services in ring 3: fsd owns the initrd, rtcd owns the cmos ports
    long fsd = spawn("fsd", 3);
    long rtcd = spawn("rtcd", 4);
    println("[init] spawned fsd (", static_cast<u64>(fsd), ") rtcd (",
            static_cast<u64>(rtcd), ")");

    // keyboard/console live as a ring-3 process on its own event queue
    long console = spawn("consoled", 8);
    println("[init] spawned consoled (", static_cast<u64>(console), ")");

    // the veda-approach smoke: a guest behind GUEST_CREATE/VCPU_RUN
    long gd = spawn("gdemo", 5);
    println("[init] spawned gdemo (", static_cast<u64>(gd), ")");

    // the next step: a 64-bit long-mode guest behind guest_mode
    long gd64 = spawn("gdemo64", 7);
    println("[init] spawned gdemo64 (", static_cast<u64>(gd64), ")");

    // recv returns the sender pid; messages carry their own NUL
    request(fsd, "motd", 4, buf, sizeof(buf));
    buf[127] = '\0';
    println("[init] fsd says: ", buf);

    request(rtcd, "time", 4, buf, sizeof(buf));
    buf[127] = '\0';
    println("[init] rtc says: ", buf);

    // managarm-style event queue: hello posts events, we drain them with
    // plain loads and only sleep in the kernel between batches
    eqmem[4] = 0x00;             // chunk_size = 512 (LE u32)
    eqmem[5] = 0x02;
    eqmem[8] = 4;                // nchunks
    long q = eq_setup(eqmem, sizeof(eqmem));
    println("[init] event queue ", static_cast<u64>(q));
    // hello may post into our queue: hand it the POST capability
    eq_grant(q, static_cast<u64>(hello), EQ_RIGHT_POST);

    // alert semantics: eq_alert cancels a parked eq_wait (fast path here —
    // the ALERT bit is up before we ever park, wait returns 2 right away)
    eq_alert(q);
    if (eq_wait(q) == 2) {
        println("[eq] wait: alerted (wait canceled)");
    }

    char msg[32];
    u64 mn = 0;
    put_str(msg, mn, "EQ ");
    put_num(msg, mn, static_cast<u64>(q));
    msg[mn] = '\0';
    send(static_cast<u64>(hello), msg, mn + 1);

    u32 total = 0;
    u32 milestone = 0;
    while (total < 101) { // 100 inline events + 1 zero-copy gift
        total += eq_drive(q, eqmem, on_event);
        while (total >= milestone + 25 && milestone < 100) {
            milestone += 25;
            println("[eq] ", static_cast<u64>(milestone), " events in");
        }
        if (total < 101 && eq_wait(q) < 0) {
            break;
        }
    }
    println("[init] drained ", static_cast<u64>(total), " events, bye");
    if (!g_zcopy_ok) {
        println("[eq] zcopy: gift never arrived");
    }
    return 0;
}

}
