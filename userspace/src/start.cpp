#include <shizoley/syscall.hpp>

namespace shz {

int main();

[[noreturn]] void start() {
    exit(main());
}

}

extern "C" [[noreturn]] void _start() {
    shz::start();
}
