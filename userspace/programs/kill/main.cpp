// kill(1): the target pid arrives as 8 LE bytes, SYS_KILL does the rest
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace shz {

int main() {
    println("[kill] pid ", static_cast<u64>(getpid()), " waiting for a target");
    u8 req[8];
    for (;;) {
        for (u64 i = 0; i < sizeof(req); ++i) {
            req[i] = 0;
        }
        long from = recv(0, req, sizeof(req));
        if (from <= 0) {
            continue;
        }
        u64 target = 0;
        for (int i = 7; i >= 0; --i) {
            target = (target << 8) | req[i];
        }
        const u8 ok[3] = {'K', 'O', 'K'};
        const u8 no[3] = {'K', 'N', 'O'};
        if (kill(target) == 0) {
            println("[kill] target ", target, " terminated (by ",
                    static_cast<u64>(from), ")");
            send(static_cast<u64>(from), ok, 3);
        } else {
            println("[kill] target ", target, " refused (by ",
                    static_cast<u64>(from), ")");
            send(static_cast<u64>(from), no, 3);
        }
    }
}

}

