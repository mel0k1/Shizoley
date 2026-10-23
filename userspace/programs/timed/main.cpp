// timed — ring-3 timer service: the kernel gives raw PIT ticks (tm_arm
// fires one OP_TICK into our queue), we multiplex per-client sleeps from
// the mailbox ("SLP" + u64 ms) and ack each client with "WOKE".
#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace {

constexpr int MAXS = 32;

static unsigned char eqmem[48 + 4 * 256] __attribute__((aligned(8)));
static long q;
static unsigned char dstack[8192] __attribute__((aligned(16)));

struct Slep {
    u64 pid; // 0 = free slot
    u64 dl;  // absolute deadline in ticks
};

static volatile Slep tab[MAXS];
static volatile int lk;
static u64 g_armed;

void lock() {
    while (__sync_lock_test_and_set(&lk, 1)) {
        asm volatile("pause");
    }
}

void unlock() {
    __sync_lock_release(&lk);
}

u64 le64(const u8* p) {
    u64 v = 0;
    for (int i = 7; i >= 0; --i) {
        v = (v << 8) | p[i];
    }
    return v;
}

void reply(u64 pid) {
    const u8 w[4] = {'W', 'O', 'K', 'E'};
    send(pid, w, 4);
}

// must hold the lock: arm the kernel one-shot for the earliest future deadline
void arm_earliest(u64 now) {
    u64 best = 0;
    for (int i = 0; i < MAXS; ++i) {
        if (tab[i].pid != 0 && tab[i].dl > now && (best == 0 || tab[i].dl < best)) {
            best = tab[i].dl;
        }
    }
    if (best != 0 && best != g_armed && tm_arm(best) == 0) {
        g_armed = best;
    }
}

void fire(u64 now) {
    u64 due[MAXS];
    int nd = 0;
    lock();
    g_armed = 0;
    for (int i = 0; i < MAXS; ++i) {
        if (tab[i].pid != 0 && tab[i].dl <= now) {
            due[nd++] = tab[i].pid;
            tab[i].pid = 0;
        }
    }
    arm_earliest(now);
    unlock();
    if (nd != 0) {
        println("[timed] fired at tick ", now, ": woke ", static_cast<u64>(nd));
    }
    for (int i = 0; i < nd; ++i) {
        reply(due[i]); // send may fail for a dead client, that is fine
    }
}

void on_event(const EqEvent& ev) {
    if (ev.opcode == EQ_OP_BOOT) {
        println("[timed] up: kernel timer sink attached (pid ",
                static_cast<u64>(getpid()), ")");
        return;
    }
    if (ev.opcode == EQ_OP_TICK && ev.len >= 8) {
        fire(le64(ev.payload));
    }
}

extern "C" void drive(u64) {
    for (;;) {
        eq_drive(q, eqmem, on_event);
        if (eq_wait(q) < 0) {
            for (int i = 0; i < 1000; ++i) {
                asm volatile("pause");
            }
        }
    }
}

}  // namespace

namespace shz {

int main() {
    println("[timed] pid ", static_cast<u64>(getpid()), ": sleep multiplexer");
    eqmem[4] = 0x00; // chunk_size = 256 (LE u32)
    eqmem[5] = 0x01;
    eqmem[8] = 4;    // nchunks
    q = eq_setup(eqmem, sizeof(eqmem));
    if (q < 0) {
        println("[timed] eq_setup failed");
        return 1;
    }
    if (eq_tlisten(q) != 0) {
        println("[timed] timer sink denied (no LISTEN right?)");
        return 1;
    }
    thread_create(drive, dstack + sizeof(dstack), 0);

    u8 req[64];
    for (;;) {
        for (u64 i = 0; i < sizeof(req); ++i) {
            req[i] = 0;
        }
        long from = recv(0, req, sizeof(req));
        if (from <= 0) {
            continue;
        }
        if (req[0] != 'S' || req[1] != 'L' || req[2] != 'P') {
            println("[timed] bad request from ", static_cast<u64>(from));
            continue;
        }
        u64 ms = le64(req + 3);
        u64 now = ticks();
        if (ms == 0) {
            reply(static_cast<u64>(from));
            continue;
        }
        u64 dl = now + (ms + 9) / 10; // 100 Hz clock
        bool added = false;
        lock();
        for (int i = 0; i < MAXS; ++i) {
            if (tab[i].pid == 0) {
                tab[i].pid = static_cast<u64>(from);
                tab[i].dl = dl;
                added = true;
                break;
            }
        }
        arm_earliest(now);
        unlock();
        if (!added) {
            reply(static_cast<u64>(from)); // table full: degenerate wake-up
        }
    }
}

}

