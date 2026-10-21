#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace shz {

int main() {
    char buf[64];
    long from = recv(0, buf, sizeof(buf));
    println("[hello] pid ", static_cast<u64>(getpid()), " got '", buf,
            "' from ", static_cast<u64>(from));
    send(static_cast<u64>(from), "pong", 5);
    println("[hello] replied, bye");
    return 0;
}

}
