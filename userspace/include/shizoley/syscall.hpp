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
    eq_setup = 8,      // (mem, size) -> root capability handle; unguessable
    eq_post = 9,       // (handle, opcode, buf, len) -> 0 | -1
    eq_wait = 10,      // (handle) -> 1 | -1; parks until the doorbell
    eq_kick = 11,      // (handle) -> 0 | -1; release chunk credit
    initrd_size = 12,  // () -> bytes (initrd mapped ro at INITRD_BASE)
    write = 13,        // (buf, len) -> len; atomic console line
    kill = 14,         // (pid) -> 0 | -1; end every thread of the process
    guest_create = 15, // (pages) -> handle; vmcs + ept + ram window
    vcpu_run = 16,     // (handle, out u64[20]) -> exit reason
    guest_destroy = 17,// (handle) -> 0 | -1; frees ram, ept, vmcs
    eq_listen = 18,    // (handle) -> 0 | -1; register kernel event sink
    eq_grant = 19,     // (handle, pid, rights) -> child handle | -1
    eq_revoke = 20,    // (handle) -> n revoked | -1; cascade the subtree
    guest_map = 21,    // (handle, count) -> new size | -1; grow guest ram+ept
    eq_alert = 22,     // (handle) -> 0 | -1; cancel a parked eq_wait
    eq_zpost = 23,     // (handle, buf, len) -> gift handle | -1; zero-copy
    eq_zmap = 24,      // (handle) -> va | -1; map the gift into the owner
    eq_zdone = 25,     // (handle) -> 0 | -1; consume the gift, free frames
    guest_mode = 26,   // (handle, mode) -> 0 | -1; mode 1 = long-mode guest
    guest_pi = 27,     // (handle, vector) -> 0 | -1; route host irq into
                       // the guest's posted-interrupt descriptor (0 = clear)
    ps = 28,           // (buf, max_entries) -> n; 40-byte process records
    ticks = 29,        // () -> scheduler tick count (PIT clock)
    tm_arm = 30,       // (deadline_ticks) -> 0 | -1; one-shot timer fire
                       // into the timer sink (owner only; 0 disarms)
};

// ps(28) record, filled by the kernel (name is NUL-padded)
struct PsEntry {
    u64 pid;
    u64 thr;
    u64 state; // 0 = run, 1 = dead (waits for the reaper)
    char name[16];
};
static_assert(sizeof(PsEntry) == 40, "ps record abi");

// vcpu_run exit reasons (reportable vmx exits; see kernel vmx.rs)
enum class VmxExit : long {
    ext_int = 1,       // host irq stopped the guest: timer tick (re-enter) or
                       // a routed vector posted into the guest (out[3]=vector)
    ud = 10,           // guest took #UD (intercepted): e.g. the rax proof failed
    vmcall = 18,       // guest hypercall (kernel advanced rip past it)
    io = 30,           // guest touched a real port: kernel blocks it (out[3]=port)
    apic_access = 44,  // guest hit the virtual apic page (out[3]=offset)
    ept_violation = 49,
    ept_misconfig = 50,
    preempt_timer = 52, // vmx preemption timer expired: re-enter
    hlt = 12,
};

// two-way vcpu exit buffer, 352 bytes: the kernel writes `out` (guest ->
// host), ring 3 fills the rest and calls vcpu_run again (host -> guest)
struct VcpuExit {
    u64 out[20];      // reason, rip, qual, gpa/port, 16 gprs
    u64 flags;        // EXIT_IN_* below
    u64 inject;       // raw vm-entry interruption-info field (sdm table 25-x)
    u64 err;          // exception error code, valid when inject bit 11 set
    u64 instr_len;    // guest instruction length (exceptions need non-zero)
    u64 entry_rip;    // entry state, loaded when EXIT_IN_ENTRY (long mode)
    u64 entry_rsp;
    u64 entry_cr3;    // guest phys, validated against guest ram size
    u64 pad;
    u64 gprs[16];     // loaded into the guest when EXIT_IN_GPRS
};
static_assert(sizeof(VcpuExit) == 352, "vcpu exit abi");

constexpr u64 EXIT_IN_GPRS = 1;   // load VcpuExit::gprs into the guest
constexpr u64 EXIT_INJECT = 2;    // deliver VcpuExit::inject on next entry
constexpr u64 EXIT_IN_ENTRY = 4;  // load entry_rip/rsp/cr3 (long mode)

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
long guest_mode(long handle, u64 mode);
long guest_pi(long handle, u64 vector);
long eq_alert(long qid);
long ps(PsEntry* buf, u64 max);
u64 ticks();
long tm_arm(u64 deadline_ticks);
long ps_find(const char* name);
long tsleep(u64 ms);

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
