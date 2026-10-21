#include <shizoley/syscall.hpp>

namespace shz {

[[noreturn]] void exit(long code) {
    syscall(Sys::exit, static_cast<u64>(code));
    __builtin_unreachable();
}

}
