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

}
