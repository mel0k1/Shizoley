// uardemo — uar/stac proof: the kernel copies user memory through a
// window with fault recovery, so a bad pointer fails the syscall (write
// returns -1) instead of killing the process. live copies stay intact.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace shz {

int main() {
    println("[uardemo] up");
    const char live[] = "[uardemo] live copy ok\n";
    long n = syscall(Sys::write, reinterpret_cast<u64>(live), sizeof(live) - 1);
    if (n < 0) {
        println("[uardemo] FAIL: live write rejected (", static_cast<u64>(n), ")");
        exit(1);
    }
    long bad = syscall(Sys::write, 0xdead0000UL, 8);
    if (bad != -1) {
        println("[uardemo] FAIL: unmapped write accepted (", static_cast<u64>(bad), ")");
        exit(1);
    }
    println("[uardemo] unmapped pointer -> -1 (fault recovered)");
    long half = syscall(Sys::write, 0xffff800000000000UL, 8);
    long over = syscall(Sys::write, 0x0000800000000000UL - 2, 16);
    if (half != -1 || over != -1) {
        println("[uardemo] FAIL: kernel-half/overflow accepted (",
                static_cast<u64>(half), " ", static_cast<u64>(over), ")");
        exit(1);
    }
    println("[uardemo] kernel-half + overflow rejected, bad pointers rejected, all ok");
    exit(0);
}

}
