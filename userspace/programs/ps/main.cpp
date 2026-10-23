// ps — dumps the process table from SYS_PS(28)
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace shz {

int main() {
    PsEntry t[64];
    long n = ps(t, 64);
    if (n < 0) {
        println("[ps] ps syscall failed");
        return 1;
    }
    println("[ps] ", static_cast<u64>(n), " processes:");
    for (long i = 0; i < n; ++i) {
        println("[ps] ", t[i].pid, " ", t[i].state == 0 ? "run" : "dead", " ",
                t[i].thr, " ", t[i].name);
    }
    return 0;
}

}

