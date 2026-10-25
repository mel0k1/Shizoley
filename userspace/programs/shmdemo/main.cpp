// shmdemo — the owner side of a managarm-style memory object. the kernel
// owns 4 frames; this process maps them rw and fills a pattern, derives a
// read-only child capability and hands its token to shmpeer over the
// mailbox. nobody copies bytes: the peer loads the same physical frames.
#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace {

constexpr u64 SHM_VA = 0x10000000;
constexpr u64 BYTES = 4 * 4096;

}

namespace shz {

int main() {
    u64 me = static_cast<u64>(getpid());

    long h = shm_create(4);
    if (h < 0) {
        println("[shmdemo] shm_create failed");
        return 1;
    }
    if (shm_map(h, SHM_VA) != 0) {
        println("[shmdemo] own map failed");
        return 1;
    }
    u8* p = reinterpret_cast<u8*>(SHM_VA);
    for (u64 i = 0; i < BYTES; ++i) {
        p[i] = static_cast<u8>(i & 0xFF);
    }
    println("[shmdemo] filled ", BYTES, " bytes at ", SHM_VA);

    // a forged token must not resolve: capabilities are unguessable
    if (shm_map(h ^ 0x5A5A5A5A5A5A5A5AUL, 0x12000000) == -1) {
        println("[shmdemo] forged token denied");
    } else {
        println("[shmdemo] forged token accepted!");
        return 1;
    }

    long peer = spawn("shmpeer", 7);
    println("[shmdemo] spawned shmpeer (", static_cast<u64>(peer), ")");

    // read-only child for the peer; rights travel as 8 bytes over ipc
    long hp = eq_grant(h, static_cast<u64>(peer), SHM_RIGHT_R);
    if (hp < 0) {
        println("[shmdemo] grant to peer failed");
        return 1;
    }
    char m[11];
    m[0] = 'S';
    m[1] = 'H';
    m[2] = 'M';
    u64 tok = static_cast<u64>(hp);
    for (int i = 0; i < 8; ++i) {
        m[3 + i] = static_cast<char>(tok >> (8 * i));
    }
    send(static_cast<u64>(peer), m, sizeof(m));

    char rep[8];
    long from = recv(static_cast<u64>(peer), rep, sizeof(rep));
    if (from != peer || rep[0] != 'O' || rep[1] != 'K' || rep[2] != 'K') {
        println("[shmdemo] peer reply bad");
        return 1;
    }
    println("[shmdemo] share ok (pid ", me, ", zero-copy)");
    shm_unmap(h);
    return 0;
}

}
