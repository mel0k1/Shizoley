// shmpeer — the other side of the memory-object share: recv the delegated
// token, map the kernel-owned frames read-only, verify the pattern with
// plain loads (no syscall, no copy), unmap, ack. the producer's exit then
// tears the object down: frames home, capabilities void.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace {

constexpr u64 PEER_VA = 0x11000000;
constexpr u64 BYTES = 4 * 4096;

}

namespace shz {

int main() {
    long prod = ps_find("shmdemo");
    if (prod < 0) {
        println("[shmpeer] shmdemo not found");
        return 1;
    }
    char m[11];
    long from = recv(static_cast<u64>(prod), m, sizeof(m));
    if (from != prod || m[0] != 'S' || m[1] != 'H' || m[2] != 'M') {
        println("[shmpeer] bad handshake");
        return 1;
    }
    u64 tok = 0;
    for (int i = 10; i >= 3; --i) {
        tok = (tok << 8) | static_cast<u64>(static_cast<u8>(m[i]));
    }
    if (shm_map(tok, PEER_VA) != 0) {
        println("[shmpeer] ro map failed");
        return 1;
    }
    const u8* p = reinterpret_cast<const u8*>(PEER_VA);
    bool ok = true;
    for (u64 i = 0; i < BYTES; ++i) {
        if (p[i] != static_cast<u8>(i & 0xFF)) {
            ok = false;
            break;
        }
    }
    if (!ok) {
        println("[shmpeer] pattern MISMATCH at ", PEER_VA);
        return 1;
    }
    println("[shmpeer] verified ", BYTES, " bytes (ro map, no copies)");
    shm_unmap(tok);
    char ok3[3] = {'O', 'K', 'K'};
    send(static_cast<u64>(prod), ok3, sizeof(ok3));
    return 0;
}

}
