#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace shz {

// 8 KiB pattern buffer, page-aligned: it travels to init as a zero-copy
// gift — the pages themselves get detached from this address space
static unsigned char bigbuf[8192] __attribute__((aligned(4096)));

int main() {
    char buf[64];
    long from = recv(0, buf, sizeof(buf));
    println("[hello] pid ", static_cast<u64>(getpid()), " got '", buf,
            "' from ", static_cast<u64>(from));
    send(static_cast<u64>(from), "pong", 5);

    // wait for the event queue handle: "EQ <qid>" (recv returns sender pid)
    for (u64 i = 0; i < sizeof(buf); ++i) {
        buf[i] = 0;
    }
    recv(0, buf, sizeof(buf));
    if (buf[0] != 'E' || buf[1] != 'Q' || buf[2] != ' ') {
        println("[hello] no queue handle, bye");
        return 1;
    }
    long q = 0;
    for (int i = 3; buf[i] >= '0' && buf[i] <= '9'; ++i) {
        q = q * 10 + (buf[i] - '0');
    }
    println("[hello] posting 100 events to queue ", static_cast<u64>(q));

    // per-op caps: we hold POST only — WAIT must be denied (no park, the
    // check fires before the doorbell ever matters)
    if (eq_wait(q) == -1) {
        println("[hello] wait denied (no WAIT right): per-op caps ok");
    } else {
        println("[hello] wait should have been denied!");
        return 1;
    }

    for (u32 i = 0; i < 100; ++i) {
        char text[8];
        text[0] = 'e';
        u64 n = 1;
        u32 v = i;
        char tmp[8];
        int t = 0;
        if (v == 0) {
            tmp[t++] = '0';
        }
        while (v > 0) {
            tmp[t++] = static_cast<char>('0' + v % 10);
            v /= 10;
        }
        while (t > 0) {
            text[n++] = tmp[--t];
        }
        if (eq_post(q, 1, text, static_cast<u32>(n)) != 0) {
            println("[hello] post failed at ", static_cast<u64>(i));
            return 1;
        }
    }
    println("[hello] posted 100, bye");

    // zero-copy finale: give the pattern pages to init through the queue;
    // after this the buffer is gone from our address space — do not touch
    for (u32 i = 0; i < sizeof(bigbuf); ++i) {
        bigbuf[i] = static_cast<unsigned char>(i & 0xFF);
    }
    long h = eq_zpost(q, bigbuf, sizeof(bigbuf));
    if (h < 0) {
        println("[hello] zpost failed");
        return 1;
    }
    println("[hello] zposted ", static_cast<u64>(sizeof(bigbuf)),
            " bytes as handle ", static_cast<u64>(h));
    return 0;
}

}
