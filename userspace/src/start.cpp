#include <shizoley/syscall.hpp>

namespace shz {

int main();

[[noreturn]] void start() {
    exit(main());
}

}

extern "C" [[noreturn]] __attribute__((section(".text._start"))) void _start() {
    shz::start();
}
