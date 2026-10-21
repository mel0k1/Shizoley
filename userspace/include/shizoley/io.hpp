#pragma once

#include <shizoley/types.hpp>

namespace shz {

void putc(char c);
void print(const char* s);
void print(u64 v);
void print(i64 v);

// implemented in io.cpp on top of the write syscall
void shz_write(const char* s, u64 n);

namespace detail {

// one write syscall per line keeps concurrent output on serial readable
inline void write_all(const char* s, u64 n) {
    shz_write(s, n);
}

inline void app(char* buf, u64& n, char v) {
    if (n < 254) {
        buf[n++] = v;
    }
}

inline void app(char* buf, u64& n, const char* v) {
    while (*v != '\0' && n < 254) {
        buf[n++] = *v++;
    }
}

inline void app(char* buf, u64& n, u64 v) {
    char tmp[20];
    int i = 0;
    if (v == 0) {
        tmp[i++] = '0';
    }
    while (v > 0) {
        tmp[i++] = static_cast<char>('0' + (v % 10));
        v /= 10;
    }
    while (i > 0 && n < 254) {
        buf[n++] = tmp[--i];
    }
}

inline void app(char* buf, u64& n, i64 v) {
    if (v < 0) {
        app(buf, n, '-');
        app(buf, n, static_cast<u64>(-(v + 1)) + 1);
    } else {
        app(buf, n, static_cast<u64>(v));
    }
}

}

template <typename... Args>
void println(Args... args) {
    char buf[256];
    u64 n = 0;
    (detail::app(buf, n, args), ...);
    buf[n++] = '\n';
    detail::write_all(buf, n);
}

}
