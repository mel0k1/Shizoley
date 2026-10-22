#pragma once

#include <shizoley/types.hpp>

namespace shz {

enum class Sys : u64 {
    putc = 0,
    exit = 1,
    spawn = 2,          // (name, name_len) -> pid
    thread_create = 3,  // (entry, stack_top, arg) -> tid
    ipc_send = 4,       // (dest_pid, buf, len) -> 0 | -1
    ipc_recv = 5,       // (src_pid /*0 = any*/, buf, max) -> sender pid
    getpid = 6,
    gettid = 7,
    eq_setup = 8,      // (mem, size) -> qid; managarm-style event queue
    eq_post = 9,       // (qid, opcode, buf, len) -> 0 | -1
    eq_wait = 10,      // (qid) -> 1 | -1; parks until the doorbell
    eq_kick = 11,      // (qid) -> 0 | -1; release chunk credit
    initrd_size = 12,  // () -> bytes (initrd mapped ro at INITRD_BASE)
    write = 13,        // (buf, len) -> len; atomic console line
    kill = 14,         // (pid) -> 0 | -1; end every thread of the process
    guest_create = 15, // (pages) -> handle; vmcs + ept + ram window
    vcpu_run = 16,     // (handle, out u64[20]) -> exit reason
    guest_destroy = 17,// (handle) -> 0 | -1; frees ram, ept, vmcs
    eq_listen = 18,    // (qid) -> 0 | -1; register kernel event sink
    eq_grant = 19,     // (qid, pid, rights) -> 0 | -1
    eq_revoke = 20,    // (qid, pid) -> 0 | -1
};

[[noreturn]] void exit(long code);

long spawn(const char* name, u64 len);
long thread_create(void (*fn)(u64), void* stack_top, u64 arg);
long send(u64 pid, const void* buf, u64 len);
long recv(u64 src, void* buf, u64 max);
long getpid();
long gettid();
long kill(u64 pid);
long guest_create(u64 pages);
long vcpu_run(long handle, u64* out /* 20 u64s */);
long guest_destroy(long handle);

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

inline long syscall(Sys nr, u64 a1, u64 a2, u64 a3) {
    u64 ret = static_cast<u64>(nr);
    asm volatile("syscall" : "+a"(ret) : "D"(a1), "S"(a2), "d"(a3) : "rcx", "r11", "memory");
    return static_cast<long>(ret);
}

inline long syscall(Sys nr, u64 a1, u64 a2, u64 a3, u64 a4) {
    u64 ret = static_cast<u64>(nr);
    register u64 r10 __asm__("r10") = a4; // 4th syscall arg travels in r10
    asm volatile("syscall" : "+a"(ret) : "D"(a1), "S"(a2), "d"(a3), "r"(r10) : "rcx", "r11", "memory");
    return static_cast<long>(ret);
}

}
