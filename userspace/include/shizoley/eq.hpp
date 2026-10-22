#pragma once

#include <shizoley/types.hpp>

namespace shz {

// managarm-style event queue over shared memory: the kernel writes
// elements into chunks, the user drains them with plain loads and only
// enters the kernel to sleep (eq_wait) or to release chunk credit (eq_kick)

constexpr u32 EQ_MAGIC = 0x5A485345u;
constexpr u32 EQ_FULL = 0x80000000u;
constexpr u32 EQ_HDR = 40;       // head size
// +32 u64 cons_off: bytes already consumed inside the current open chunk
// (consumer-owned, survives across eq_drive calls)
constexpr u32 EQ_ELEM_HDR = 16;  // element header size

// capability rights on a qid (owner holds them all implicitly)
constexpr u32 EQ_RIGHT_POST = 1;
constexpr u32 EQ_RIGHT_WAIT = 2;
constexpr u32 EQ_RIGHT_KICK = 4;
constexpr u32 EQ_RIGHT_LISTEN = 8;

// opcodes the kernel posts into a listening queue
constexpr u32 EQ_OP_KEY = 1;   // payload: [scancode, ascii]
constexpr u32 EQ_OP_BOOT = 2;  // payload: "boot" — sink attached

// head layout (user memory):
// +0 magic, +4 chunk_size, +8 nchunks, +12 user_notify,
// +16 prod_seq (kernel), +24 cons_seq (user)
// chunk i at +32 + i*chunk_size: +0 reserved, +4 progress, +8 data[]
// element: +0 len, +4 opcode, +8 sender pid, +16 payload

struct EqEvent {
    u32 opcode;
    u64 sender;
    const u8* payload;
    u32 len;
};

long eq_setup(void* mem, u64 size);
long eq_post(long qid, u32 opcode, const void* data, u32 len);
long eq_wait(long qid);
long eq_kick(long qid);
long eq_listen(long qid);
long eq_grant(long qid, u64 pid, u32 rights);
long eq_revoke(long qid, u64 pid);

// drains pending elements (plain loads, no syscalls per element) and calls
// cb for each; base must be the memory given to eq_setup; releases credit
// via cons_seq++/eq_kick when a full chunk is consumed; returns the count
u32 eq_drive(long qid, void* base, void (*cb)(const EqEvent&));

}
