// capsdemo — the owner side of the full capability round trip. the queue
// handle returned by eq_setup is an unguessable token bound to this
// process; rights move as 8-byte tokens over the mailbox, delegation is
// would_cycle-checked, and revoke cascades through the derivation forest.
// the scripted round trip (B and C are two capsuser processes):
//   1. grant(h0, B, POST|GRANT) -> h1, token sent over ipc
//   2. self-grant refused (would_cycle: the chain reaches the owner)
//   3. forged grant denied (tokens are unguessable)
//   4. B delegates a POST-only child to C ("DOK"); C posts with it ("PST")
//   5. B tries to grant back to the owner — refused ("CYC")
//   6. revoke(h1) from the owner cascades: B's and C's handles die
//   7. B can no longer post ("R" denial); a fresh re-grant works ("RGD")
#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace {

constexpr u32 OP_CAPS = 30; // result opcode used by capsuser

// the queue memory: 4 chunks x 512 bytes
unsigned char eqmem[48 + 4 * 512] __attribute__((aligned(8)));

// drained results: opcode 30, payload DOK/PST/CYC/RGD, sender pid
struct Res {
    char tag[4];
    u64 from;
};
Res g_results[8];
u32 g_nresults = 0;

void on_event(const EqEvent& ev) {
    if (ev.opcode != OP_CAPS || g_nresults >= 8) {
        return;
    }
    Res& r = g_results[g_nresults];
    r.from = ev.sender;
    for (u32 i = 0; i < 3; ++i) {
        r.tag[i] = i < ev.len ? static_cast<char>(ev.payload[i]) : '\0';
    }
    r.tag[3] = '\0';
    ++g_nresults;
}

// drain until the wanted tag shows up (or the spin budget dies); returns
// the sender pid of the match, 0 on failure
u64 wait_for(long h, const char* tag) {
    for (int spin = 0; spin < 20000; ++spin) {
        eq_drive(h, eqmem, on_event);
        for (u32 i = 0; i < g_nresults; ++i) {
            if (g_results[i].tag[0] == tag[0] && g_results[i].tag[1] == tag[1]
                && g_results[i].tag[2] == tag[2]) {
                return g_results[i].from;
            }
        }
        if (eq_wait(h) < 0) {
            return 0;
        }
    }
    return 0;
}

void send_cap(long pid, long tok) {
    char m[11];
    m[0] = 'C';
    m[1] = 'A';
    m[2] = 'P';
    u64 v = static_cast<u64>(tok);
    for (int i = 0; i < 8; ++i) {
        m[3 + i] = static_cast<char>(v >> (8 * i));
    }
    send(static_cast<u64>(pid), m, sizeof(m));
}

void send_cmd(long pid, u64 owner, u64 target, char cmd, const char* tag) {
    char m[23];
    m[0] = 'C';
    m[1] = 'M';
    m[2] = 'D';
    for (int i = 0; i < 8; ++i) {
        m[3 + i] = static_cast<char>(owner >> (8 * i));
        m[11 + i] = static_cast<char>(target >> (8 * i));
    }
    m[19] = cmd;
    m[20] = tag[0];
    m[21] = tag[1];
    m[22] = tag[2];
    send(static_cast<u64>(pid), m, sizeof(m));
}

} // namespace

namespace shz {

int main() {
    u64 me = static_cast<u64>(getpid());

    eqmem[4] = 0x00; // chunk_size = 512 (LE u32)
    eqmem[5] = 0x02;
    eqmem[8] = 4; // nchunks
    long h0 = eq_setup(eqmem, sizeof(eqmem));
    if (h0 < 0) {
        println("[capsdemo] eq_setup failed");
        return 1;
    }

    long b = spawn("capsuser", 8);
    long c = spawn("capsuser", 8);
    println("[capsdemo] users ", static_cast<u64>(b), " ",
            static_cast<u64>(c));

    // 1. delegate-with-regrant right to B; the token travels as bytes
    long h1 = eq_grant(h0, static_cast<u64>(b),
                       EQ_RIGHT_POST | EQ_RIGHT_GRANT);
    if (h1 < 0) {
        println("[capsdemo] grant to B failed");
        return 1;
    }
    send_cap(b, h1);

    // 2. self-grant: the derivation chain already reaches the owner
    if (eq_grant(h0, me, EQ_RIGHT_POST) == -1) {
        println("[capsdemo] self-grant refused (would_cycle ok)");
    } else {
        println("[capsdemo] self-grant accepted!");
        return 1;
    }

    // 3. a forged token must not resolve at all
    if (eq_grant(h0 ^ 0x5A5A5A5A5A5A5A5AUL, static_cast<u64>(c),
                 EQ_RIGHT_POST) == -1) {
        println("[capsdemo] forged grant denied (unguessable ok)");
    } else {
        println("[capsdemo] forged grant accepted!");
        return 1;
    }

    // 4. B delegates a POST-only child to C, C proves it with a post
    send_cmd(b, me, static_cast<u64>(c), 'D', "---");
    if (wait_for(h0, "DOK") == static_cast<u64>(b)) {
        println("[capsdemo] delegation ok");
    } else {
        println("[capsdemo] delegation failed");
        return 1;
    }
    send_cmd(c, me, 0, 'P', "PST");
    if (wait_for(h0, "PST") == static_cast<u64>(c)) {
        println("[capsdemo] delegated post ok (from C)");
    } else {
        println("[capsdemo] delegated post failed");
        return 1;
    }

    // 5. B grants back to the owner: would_cycle must refuse
    send_cmd(b, me, 0, 'C', "---");
    if (wait_for(h0, "CYC") == static_cast<u64>(b)) {
        println("[capsdemo] would_cycle refused the grant-back");
    } else {
        println("[capsdemo] cycle was accepted!");
        return 1;
    }

    // 6. revoke from the owner: the B->C subtree dies (both handles)
    long nrev = eq_revoke(h1);
    println("[capsdemo] revoked ", static_cast<u64>(nrev),
            " derived handles");
    if (nrev != 2) {
        println("[capsdemo] revoke cascade wrong");
        return 1;
    }

    // 7. B's revoked token must not post anymore; a fresh grant does
    send_cmd(b, me, 0, 'R', "---");
    long h3 = eq_grant(h0, static_cast<u64>(b), EQ_RIGHT_POST);
    if (h3 < 0) {
        println("[capsdemo] re-grant failed");
        return 1;
    }
    send_cap(b, h3);
    send_cmd(b, me, 0, 'P', "RGD");
    if (wait_for(h0, "RGD") == static_cast<u64>(b)) {
        println("[capsdemo] re-grant after revoke ok");
    } else {
        println("[capsdemo] re-grant post failed");
        return 1;
    }

    // wind the helpers down
    send_cmd(b, me, 0, 'Q', "---");
    send_cmd(c, me, 0, 'Q', "---");

    println("[capsdemo] all ok");
    return 0;
}

} // namespace shz
