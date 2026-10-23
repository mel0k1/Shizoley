// fuxdemo — futex over the doorbell: threads of one process synchronize
// on a plain u32 in their own memory. the waiter proves both paths: a
// mismatched value returns 1 without ever parking, the matched value
// parks in the kernel until the setter's futex_wake releases it.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

static u32 word = 0;
static u32 ack = 0;
static unsigned char stack1[8192] __attribute__((aligned(16)));

extern "C" void waiter(u64) {
    long fast = futex_wait(&word, 0xDEAD);
    if (fast != 1) {
        println("[futex] mismatch path broken (", static_cast<u64>(fast), ")");
        exit(1);
    }
    println("[futex] mismatch returned 1 (no sleep)");
    while (word == 0) {
        if (futex_wait(&word, 0) < 0) {
            println("[futex] wait failed");
            exit(1);
        }
    }
    println("[futex] woke, word ", static_cast<u64>(word));
    ack = 1;
    futex_wake(&ack, 1);
    exit(0);
}

namespace shz {

int main() {
    println("[futex] up");
    long t = thread_create(waiter, stack1 + sizeof(stack1), 0);
    if (t < 0) {
        println("[futex] thread_create failed");
        return 1;
    }
    // a beat so the waiter reaches the park before the wake lands
    for (u64 spin = 0; spin < 2000000; ++spin) {
        asm volatile("pause");
    }
    word = 42;
    futex_wake(&word, 1);
    while (ack == 0) {
        futex_wait(&ack, 0);
    }
    println("[futex] all ok");
    return 0;
}

} // namespace shz
