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

long guest_pi(long handle, u64 vector) {
    return syscall(Sys::guest_pi, static_cast<u64>(handle), vector);
}

}
