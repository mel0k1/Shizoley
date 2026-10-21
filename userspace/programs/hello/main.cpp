#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace shz {

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
    return 0;
}

}
