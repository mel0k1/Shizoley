// capsuser — the grantee side of the capability round trip. the protocol
// (all messages via mailbox ipc):
//   msg1: 'C','A','P' + 8-byte LE queue capability token (from the owner
//         directly, or from a delegating peer)
//   msg2: 'C','M','D' + u64 owner pid + u64 target pid + cmd char
//     'D' — delegate: derive a POST-only child for target_pid and send it
//           the 'CAP' message; report "DOK"/"DNO" through the queue
//     'C' — cycle attempt: try to grant back to the owner; would_cycle
//           must refuse it; report "CYC" (refused, good) or "COK" (bad)
//     'P' — plain post: prove the delegated POST right; report "PST"
//     'R' — post-after-revoke: the token died upstream, the post must be
//           denied; says so on the console
//     'Q' — quit: the round trip is over
// results travel as queue events, opcode 30, so the owner just drains.
#include <shizoley/eq.hpp>
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>

using namespace shz;

namespace {

constexpr u32 OP_CAPS = 30; // capsdemo result opcode

bool take_token(const char* buf, u64 n, long& tok) {
    if (n < 11 || buf[0] != 'C' || buf[1] != 'A' || buf[2] != 'P') {
        return false;
    }
    u64 v = 0;
    for (int i = 0; i < 8; ++i) {
        v |= static_cast<u64>(static_cast<unsigned char>(buf[3 + i])) << (8 * i);
    }
    tok = static_cast<long>(v);
    return true;
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

void report(long tok, const char* what, u64 n) {
    eq_post(tok, OP_CAPS, what, static_cast<u32>(n));
}

} // namespace

namespace shz {

int main() {
    char buf[32];
    long tok = 0;
    if (recv(0, buf, sizeof(buf)) < 0 || !take_token(buf, sizeof(buf), tok)) {
        println("[capsuser] no capability, bye");
        return 1;
    }

    // command loop: the owner drives several steps ('D', 'C', 'R', 'P')
    for (;;) {
        if (recv(0, buf, sizeof(buf)) < 0) {
            println("[capsuser] recv failed, bye");
            return 1;
        }
        if (buf[0] == 'C' && buf[1] == 'A' && buf[2] == 'P') {
            // a fresh capability (e.g. after a revoke + re-grant)
            if (!take_token(buf, sizeof(buf), tok)) {
                println("[capsuser] malformed capability");
                return 1;
            }
            continue;
        }
        if (buf[0] != 'C' || buf[1] != 'M' || buf[2] != 'D') {
            println("[capsuser] bad command framing");
            return 1;
        }
        u64 owner = 0;
        u64 target = 0;
        for (int i = 0; i < 8; ++i) {
            owner |= static_cast<u64>(static_cast<unsigned char>(buf[3 + i])) << (8 * i);
            target |= static_cast<u64>(static_cast<unsigned char>(buf[11 + i])) << (8 * i);
        }
        char cmd = buf[19];

        if (cmd == 'Q') {
            return 0;
        }
        if (cmd == 'D') {
            // delegation: POST-only child for the target, token travels
            // as bytes
            long child = eq_grant(tok, target, EQ_RIGHT_POST);
            if (child < 0) {
                report(tok, "DNO", 3);
                return 1;
            }
            send_cap(target, child);
            report(tok, "DOK", 3);
            continue;
        }
        if (cmd == 'C') {
            // the cycle attempt: granting back to a holder of an ancestor
            // must be refused by the kernel's would_cycle check
            long back = eq_grant(tok, owner, EQ_RIGHT_POST);
            if (back < 0) {
                report(tok, "CYC", 3);
                continue;
            }
            report(tok, "COK", 3); // cycle accepted — must never happen
            return 1;
        }
        if (cmd == 'P') {
            // the owner picks the tag ("PST" first proof, "RGD" after a
            // revoke + re-grant) so it can tell the runs apart
            const char* tag = &buf[20];
            report(tok, tag, 3);
            continue;
        }
        if (cmd == 'R') {
            // the token was revoked upstream: the post must be denied
            if (eq_post(tok, OP_CAPS, "RIP", 3) == -1) {
                println("[capsuser] post denied after revoke (cascade ok)");
                continue;
            }
            println("[capsuser] revoked token still posts!");
            return 1;
        }
        println("[capsuser] unknown cmd '", cmd, "'");
        return 1;
    }
}

} // namespace shz
