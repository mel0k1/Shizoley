#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

static unsigned char stack1[8192] __attribute__((aligned(16)));
static unsigned char stack2[8192] __attribute__((aligned(16)));

// user thread entry: the kernel puts arg in rdi, so this matches the SysV ABI
extern "C" void worker(u64 arg) {
    for (u64 i = 0; i < 3; ++i) {
        println("[t", arg, "] tick ", i);
    }
    exit(0);
}

namespace shz {

int main() {
    println("[init] pid ", static_cast<u64>(getpid()));

    long t1 = thread_create(worker, stack1 + sizeof(stack1), 1);
    long t2 = thread_create(worker, stack2 + sizeof(stack2), 2);
    println("[init] threads: ", static_cast<u64>(t1), " ", static_cast<u64>(t2));

    long hello = spawn("hello", 5);
    println("[init] spawned hello (", static_cast<u64>(hello), ")");

    send(static_cast<u64>(hello), "ping", 5);

    char buf[64];
    long from = recv(static_cast<u64>(hello), buf, sizeof(buf));
    println("[init] ", buf, " from ", static_cast<u64>(from));
    return 0;
}

}
