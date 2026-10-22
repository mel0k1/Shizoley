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
    guest_map = 21,    // (handle, count) -> new size | -1; grow guest ram+ept
    eq_alert = 22,     // (qid) -> 0 | -1; cancel a parked eq_wait
};

// vcpu_run exit reasons (reportable vmx exits; see kernel vmx.rs)
enum class VmxExit : long {
    ext_int = 1,       // timer tick stopped the guest for scheduling: re-enter
    vmcall = 18,       // guest hypercall (kernel advanced rip past it)
    io = 30,           // guest touched a real port: kernel blocks it (out[3]=port)
    apic_access = 44,  // guest hit the virtual apic page (out[3]=offset)
    ept_violation = 49,
    ept_misconfig = 50,
    preempt_timer = 52, // vmx preemption timer expired: re-enter
    hlt = 12,
};

// two-way vcpu exit buffer, 320 bytes: the kernel writes `out` (guest ->
// host), ring 3 fills the rest and calls vcpu_run again (host -> guest)
struct VcpuExit {
    u64 out[20];   // reason, rip, qual, gpa/port, 16 gprs
    u64 flags;     // EXIT_IN_* below
    u64 inject;    // raw vm-entry interruption-info field (sdm table 25-x)
    u64 err;       // exception error code, valid when inject bit 11 set
    u64 instr_len; // guest instruction length (exceptions need non-zero)
    u64 gprs[16];  // loaded into the guest when EXIT_IN_GPRS
};
static_assert(sizeof(VcpuExit) == 320, "vcpu exit abi");

constexpr u64 EXIT_IN_GPRS = 1;  // load VcpuExit::gprs into the guest
constexpr u64 EXIT_INJECT = 2;   // deliver VcpuExit::inject on next entry

// injection builders (raw sdm vm-entry interruption-info field):
// bits 7:0 vector, 10:8 type (0 extint, 2 nmi, 3 exception), 11 err-valid,
// 31 valid. the error code and instruction length travel in the separate
// VcpuExit fields; the kernel validates and programs the vmcs from all three
constexpr u64 vexit_extint(u64 vector) { return 0x80000000u | vector; }
constexpr u64 vexit_exception(u64 vector) {
    return 0x80000000u | (3u << 8) | (1u << 11) | vector;
}

[[noreturn]] void exit(long code);

long spawn(const char* name, u64 len);
long thread_create(void (*fn)(u64), void* stack_top, u64 arg);
long send(u64 pid, const void* buf, u64 len);
long recv(u64 src, void* buf, u64 max);
long getpid();
long gettid();
long kill(u64 pid);
long guest_create(u64 pages);
long guest_map(long handle, u64 count);
long vcpu_run(long handle, VcpuExit* ve);
long guest_destroy(long handle);
long eq_alert(long qid);

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
