// rtcd — hardware clock driver in ring 3: owns the CMOS RTC (ports
// 0x70/0x71, reachable thanks to IOPL=3) and answers "time" requests
// over the mailbox ipc with an HH:MM:SS string.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace {

inline void outb(u16 port, u8 v) {
    asm volatile("out %0, %1" : : "a"(v), "Nd"(port));
}

inline u8 inb(u16 port) {
    u8 v;
    asm volatile("in %1, %0" : "=a"(v) : "Nd"(port));
    return v;
}

inline u8 cmos(u8 reg) {
    outb(0x70, reg);
    return inb(0x71);
}

u8 from_bcd(u8 v) {
    return static_cast<u8>((v & 0x0F) + (v >> 4) * 10);
}

// reads the time of day, waiting out the update-in-progress flag
bool read_time(u8& h, u8& m, u8& s) {
    // update in progress: status register A bit 7
    for (int i = 0; i < 100000 && (cmos(0x0A) & 0x80) != 0; ++i) {
    }
    if ((cmos(0x0A) & 0x80) != 0) {
        return false;
    }
    s = cmos(0x00);
    m = cmos(0x02);
    h = cmos(0x04);
    // status B bit 1: 24h mode; bit 2: binary (not BCD)
    u8 status_b = cmos(0x0B);
    if ((status_b & 0x04) == 0) {
        s = from_bcd(s);
        m = from_bcd(m);
        h = from_bcd(h & 0x7F);
    }
    if (h >= 24 || m >= 60 || s >= 60) {
        return false;
    }
    return true;
}

char hex1(u8 v) {
    return static_cast<char>('0' + v);
}

}

namespace shz {

int main() {
    u8 h = 0;
    u8 m = 0;
    u8 s = 0;
    println("[rtcd] pid ", static_cast<u64>(getpid()), " owns cmos ports 0x70/0x71");
    char req[64];
    char reply[16];
    for (;;) {
        for (u64 i = 0; i < sizeof(req); ++i) {
            req[i] = 0;
        }
        long from = recv(0, req, sizeof(req));
        if (from <= 0) {
            continue;
        }
        if (!read_time(h, m, s)) {
            const char* err = "ERR:rtc busy";
            u64 n = 0;
            while (err[n] != '\0') {
                ++n;
            }
            send(static_cast<u64>(from), err, n);
            continue;
        }
        reply[0] = hex1(h / 10);
        reply[1] = hex1(h % 10);
        reply[2] = ':';
        reply[3] = hex1(m / 10);
        reply[4] = hex1(m % 10);
        reply[5] = ':';
        reply[6] = hex1(s / 10);
        reply[7] = hex1(s % 10);
        reply[8] = '\0';
        send(static_cast<u64>(from), reply, 9);
        println("[rtcd] time ", static_cast<u64>(h), ':', static_cast<u64>(m), ':',
                static_cast<u64>(s), " -> ", static_cast<u64>(from));
    }
}

}
