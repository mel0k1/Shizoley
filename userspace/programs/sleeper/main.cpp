// sleeper — two naps through the ring-3 timer daemon; init kills us in
// the middle of the second one
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace shz {

int main() {
    println("[sleeper] pid ", static_cast<u64>(getpid()), " napping 300ms");
    u64 t0 = ticks();
    if (tsleep(300) != 0) {
        println("[sleeper] timer daemon failed");
        return 1;
    }
    println("[sleeper] slept 300ms (measured ", (ticks() - t0) * 10, "ms)");
    println("[sleeper] nap 2: 5000ms (will not finish)");
    if (tsleep(5000) == 0) {
        println("[sleeper] nap 2 done?!");
    }
    return 0;
}

}

