#pragma once

#include <shizoley/types.hpp>

namespace shz {

void putc(char c);
void print(const char* s);
void print(u64 v);
void print(i64 v);

namespace detail {

inline void put(char v) { putc(v); }
inline void put(const char* v) { print(v); }
inline void put(u64 v) { print(v); }
inline void put(i64 v) { print(v); }

}

template <typename... Args>
void println(Args... args) {
    (detail::put(args), ...);
    putc('\n');
}

}
