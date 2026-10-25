#include <shizoley/syscall.hpp>

namespace shz {

[[noreturn]] void exit(long code) {
    syscall(Sys::exit, static_cast<u64>(code));
    __builtin_unreachable();
}

long spawn(const char* name, u64 len) {
    return syscall(Sys::spawn, reinterpret_cast<u64>(name), len);
}

long thread_create(void (*fn)(u64), void* stack_top, u64 arg) {
    return syscall(Sys::thread_create, reinterpret_cast<u64>(fn),
                   reinterpret_cast<u64>(stack_top), arg);
}

long send(u64 pid, const void* buf, u64 len) {
    return syscall(Sys::ipc_send, pid, reinterpret_cast<u64>(buf), len);
}

long recv(u64 src, void* buf, u64 max) {
    return syscall(Sys::ipc_recv, src, reinterpret_cast<u64>(buf), max);
}

long getpid() {
    return syscall(Sys::getpid);
}

long gettid() {
    return syscall(Sys::gettid);
}

long kill(u64 pid) {
    return syscall(Sys::kill, pid);
}

long guest_create(u64 pages) {
    return syscall(Sys::guest_create, pages);
}

long guest_map(long handle, u64 count) {
    return syscall(Sys::guest_map, static_cast<u64>(handle), count);
}

long vcpu_run(long handle, VcpuExit* ve) {
    return syscall(Sys::vcpu_run, static_cast<u64>(handle),
                   reinterpret_cast<u64>(ve));
}

long guest_destroy(long handle) {
    return syscall(Sys::guest_destroy, static_cast<u64>(handle));
}

long guest_mode(long handle, u64 mode) {
    return syscall(Sys::guest_mode, static_cast<u64>(handle), mode);
}

long guest_device_attach(long handle, u64 sid) {
    return syscall(Sys::guest_device_attach, static_cast<u64>(handle), sid);
}

long guest_device_detach(long handle, u64 sid) {
    return syscall(Sys::guest_device_detach, static_cast<u64>(handle), sid);
}

long guest_mmio(long handle, u64 sid, u64 bar) {
    return syscall(Sys::guest_mmio, static_cast<u64>(handle), sid, bar);
}

long guest_pi(long handle, u64 vector) {
    return syscall(Sys::guest_pi, static_cast<u64>(handle), vector);
}

long ps(PsEntry* buf, u64 max) {
    return syscall(Sys::ps, reinterpret_cast<u64>(buf), max);
}

u64 ticks() {
    return static_cast<u64>(syscall(Sys::ticks));
}

long tm_arm(u64 deadline_ticks) {
    return syscall(Sys::tm_arm, deadline_ticks);
}

long ps_find(const char* name) {
    PsEntry t[64];
    long n = ps(t, 64);
    for (long i = 0; i < n; ++i) {
        const char* a = t[i].name;
        const char* b = name;
        while (*b != '\0' && *a == *b) {
            ++a;
            ++b;
        }
        if (*b == '\0' && *a == '\0') {
            return t[i].pid;
        }
    }
    return -1;
}

// sleep ms through the ring-3 timer daemon: "SLP" + u64 ms over the mailbox
long tsleep(u64 ms) {
    long pid = ps_find("timed");
    if (pid < 0) {
        return -1;
    }
    u8 msg[11];
    msg[0] = 'S';
    msg[1] = 'L';
    msg[2] = 'P';
    for (int i = 0; i < 8; ++i) {
        msg[3 + i] = static_cast<u8>((ms >> (8 * i)) & 0xFF);
    }
    if (send(static_cast<u64>(pid), msg, sizeof(msg)) != 0) {
        return -1;
    }
    u8 rep[8];
    long from = recv(static_cast<u64>(pid), rep, sizeof(rep));
    if (from != pid || rep[0] != 'W' || rep[1] != 'O' || rep[2] != 'K' || rep[3] != 'E') {
        return -1;
    }
    return 0;
}

long futex_wait(u32* uaddr, u32 expected) {
    return syscall(Sys::futex_wait, reinterpret_cast<u64>(uaddr), expected);
}

long futex_wake(u32* uaddr, u64 n) {
    return syscall(Sys::futex_wake, reinterpret_cast<u64>(uaddr), n);
}

long shm_create(u64 npages) {
    return syscall(Sys::shm_create, npages);
}

long shm_map(long handle, u64 va) {
    return syscall(Sys::shm_map, static_cast<u64>(handle), va);
}

long shm_unmap(long handle) {
    return syscall(Sys::shm_unmap, static_cast<u64>(handle));
}

}
