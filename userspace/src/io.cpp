#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

namespace shz {

// one write syscall per line keeps concurrent output on serial readable
void shz_write(const char* s, u64 n) {
    syscall(Sys::write, reinterpret_cast<u64>(s), n);
}

void putc(char c) {
    syscall(Sys::putc, static_cast<u64>(static_cast<unsigned char>(c)));
}

void print(const char* s) {
    if (s == nullptr) {
        return;
    }
    u64 n = 0;
    while (s[n] != '\0') {
        ++n;
    }
    shz_write(s, n);
}

void print(u64 v) {
    char buf[20];
    int i = 20;
    if (v == 0) {
        buf[--i] = '0';
    }
    while (v > 0) {
        buf[--i] = static_cast<char>('0' + (v % 10));
        v /= 10;
    }
    shz_write(buf + i, static_cast<u64>(20 - i));
}

void print(i64 v) {
    if (v < 0) {
        putc('-');
        print(static_cast<u64>(-(v + 1)) + 1);
    } else {
        print(static_cast<u64>(v));
    }
}

}
