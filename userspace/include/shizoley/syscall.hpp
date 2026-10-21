#pragma once

#include <shizoley/types.hpp>

namespace shz {

enum class Sys : u64 {
    putc = 0,
    exit = 1,
};

[[noreturn]] void exit(long code);

inline long syscall(Sys nr) {
    u64 ret = static_cast<u64>(nr);
    asm volatile("syscall" : "+a"(ret) : : "rcx", "r11", "memory");
    return static_cast<long>(ret);
}

inline long syscall(Sys nr, u64 a1) {
    u64 ret = static_cast<u64>(nr);
    asm volatile("syscall" : "+a"(ret) : "D"(a1) : "rcx", "r11", "memory");
    return static_cast<long>(ret);
}

inline long syscall(Sys nr, u64 a1, u64 a2) {
    u64 ret = static_cast<u64>(nr);
    asm volatile("syscall" : "+a"(ret) : "D"(a1), "S"(a2) : "rcx", "r11", "memory");
    return static_cast<long>(ret);
}

}
