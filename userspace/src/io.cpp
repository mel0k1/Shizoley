#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

namespace shz {

void putc(char c) {
    syscall(Sys::putc, static_cast<u64>(static_cast<unsigned char>(c)));
}

void print(const char* s) {
    if (s == nullptr) {
        return;
    }
    while (*s != '\0') {
        putc(*s);
        ++s;
    }
}

void print(u64 v) {
    if (v == 0) {
        putc('0');
        return;
    }
    char buf[20];
    int i = 20;
    while (v > 0) {
        buf[--i] = static_cast<char>('0' + (v % 10));
        v /= 10;
    }
    while (i < 20) {
        putc(buf[i]);
        ++i;
    }
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
