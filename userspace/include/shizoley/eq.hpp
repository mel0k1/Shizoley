#pragma once

#include <shizoley/types.hpp>

namespace shz {

// managarm-style event queue v2 over shared memory: the kernel writes
// elements into chained chunks, the user drains them with plain loads and
// only enters the kernel to sleep (eq_wait), to release chunk credit
// (eq_kick) or to cancel a parked wait (eq_alert)

constexpr u32 EQ_MAGIC = 0x5A485632u; // "ZHv2"
constexpr u32 EQ_FULL = 0x80000000u;
constexpr u32 EQ_HDR = 48;      // head size
// +32 u32 error_code (kernel: first channel error), +36 u32 err_count
// +40 u64 cons_off: bytes already consumed inside the current open chunk
// (consumer-owned, survives across eq_drive calls)
constexpr u32 EQ_ELEM_HDR = 16; // element header size

// user_notify bits at head+12 (kernel sets, user clears)
constexpr u32 EQ_NOTIFY_EVENT = 1;
constexpr u32 EQ_NOTIFY_ALERT = 2;
constexpr u32 EQ_NOTIFY_ERROR = 4;

// kernel error codes (head+32), read and cleared via eq_errors()
constexpr u32 EQ_E_DROP = 1;   // kernel dropped an event (credit shortage)
constexpr u32 EQ_E_CLOSED = 2; // queue closed, owner gone
constexpr u32 EQ_E_PROTO = 3;  // producer protocol violation

// capability rights on a queue handle (the owner's root handle carries
// them all plus GRANT for delegation)
constexpr u32 EQ_RIGHT_POST = 1;
constexpr u32 EQ_RIGHT_WAIT = 2;
constexpr u32 EQ_RIGHT_KICK = 4;
constexpr u32 EQ_RIGHT_LISTEN = 8;
constexpr u32 EQ_RIGHT_SIGNAL = 16;
constexpr u32 EQ_RIGHT_GRANT = 32;

// opcodes the kernel posts into a listening queue
constexpr u32 EQ_OP_KEY = 1;   // payload: [scancode, ascii]
constexpr u32 EQ_OP_BOOT = 2;  // payload: "boot" — sink attached
constexpr u32 EQ_OP_ERROR = 3; // reserved for error-channel users
constexpr u32 EQ_OP_ZMSG = 4;  // zero-copy gift: payload = u64 handle + u64 bytes
constexpr u32 EQ_OP_TICK = 5;  // timer fire: payload = u64 tick count

// head layout (user memory):
// +0 magic, +4 chunk_size, +8 nchunks, +12 user_notify,
// +16 prod_seq (kernel), +24 cons_seq (user), +32 error_code,
// +36 err_count, +40 cons_off (user)
// chunk i at +48 + i*chunk_size: +0 next (producer chain), +4 progress,
// +8 data[]
// element: +0 len, +4 opcode, +8 sender pid, +16 payload

struct EqEvent {
    u32 opcode;
    u64 sender;
    const u8* payload;
    u32 len;
};

long eq_setup(void* mem, u64 size);
long eq_post(long handle, u32 opcode, const void* data, u32 len);
long eq_wait(long handle);
long eq_alert(long handle);
long eq_kick(long handle);
long eq_listen(long handle);
// attach the queue as the TIMER sink (eq_listen which=1): the kernel fires
// one OP_TICK with the tick count whenever the tm_arm deadline passes
long eq_tlisten(long handle);
// derive a child capability for pid (subset of the caller's rights; the
// caller's handle must carry GRANT) and return its token — pass it to pid
// over ipc; refused when the derivation chain would cycle (would_cycle)
long eq_grant(long handle, u64 pid, u32 rights);
// take the whole derivation subtree back; returns how many handles died
long eq_revoke(long handle);

// zero-copy big events: the poster gives away the pages behind a
// page-aligned buffer (eq_zpost -> handle), the queue owner maps them with
// eq_zmap (plain loads, no copies) and frees them with eq_zdone. a gift
// dies with either side of the queue.
long eq_zpost(long handle, const void* buf, u64 len);
long eq_zmap(long handle);
long eq_zdone(long handle);

// drains pending elements (plain loads, no syscalls per element) and calls
// cb for each; follows the producer's chunk chain and returns chunks by
// zeroing their progress word; releases credit via cons_seq++/eq_kick;
// returns the count
u32 eq_drive(long handle, void* base, void (*cb)(const EqEvent&));

// error channel: returns the number of errors raised so far and (if the
// ERROR bit is up) stores the first error code; clears the bit
u32 eq_errors(void* base, u32* code);

}
