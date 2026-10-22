#include <shizoley/eq.hpp>
#include <shizoley/syscall.hpp>

namespace shz {

namespace {

inline u32 ld32(const volatile u8* p) {
    return static_cast<u32>(p[0]) | (static_cast<u32>(p[1]) << 8) |
           (static_cast<u32>(p[2]) << 16) | (static_cast<u32>(p[3]) << 24);
}

inline u64 ld64(const volatile u8* p) {
    u64 v = 0;
    for (int i = 7; i >= 0; --i) {
        v = (v << 8) | p[i];
    }
    return v;
}

inline void st64(volatile u8* p, u64 v) {
    for (int i = 0; i < 8; ++i) {
        p[i] = static_cast<u8>(v >> (8 * i));
    }
}

inline void st32(volatile u8* p, u32 v) {
    for (int i = 0; i < 4; ++i) {
        p[i] = static_cast<u8>(v >> (8 * i));
    }
}

inline volatile u8* head(u8* base) {
    return base;
}

inline volatile u8* chunk(u8* base, u32 chunk_size, u64 idx) {
    return base + EQ_HDR + idx * chunk_size;
}

}

long eq_setup(void* mem, u64 size) {
    volatile u8* h = static_cast<volatile u8*>(mem);
    h[0] = static_cast<u8>(EQ_MAGIC);
    h[1] = static_cast<u8>(EQ_MAGIC >> 8);
    h[2] = static_cast<u8>(EQ_MAGIC >> 16);
    h[3] = static_cast<u8>(EQ_MAGIC >> 24);
    // chunk_size / nchunks are filled by the caller before this call
    return syscall(Sys::eq_setup, reinterpret_cast<u64>(mem), size);
}

long eq_post(long qid, u32 opcode, const void* data, u32 len) {
    return syscall(Sys::eq_post, static_cast<u64>(qid),
                   static_cast<u64>(opcode), reinterpret_cast<u64>(data),
                   static_cast<u64>(len));
}

long eq_wait(long qid) {
    return syscall(Sys::eq_wait, static_cast<u64>(qid));
}

long eq_alert(long qid) {
    return syscall(Sys::eq_alert, static_cast<u64>(qid));
}

long eq_kick(long qid) {
    return syscall(Sys::eq_kick, static_cast<u64>(qid));
}

long eq_listen(long qid) {
    return syscall(Sys::eq_listen, static_cast<u64>(qid));
}

long eq_grant(long qid, u64 pid, u32 rights) {
    return syscall(Sys::eq_grant, static_cast<u64>(qid), pid, rights);
}

long eq_revoke(long qid, u64 pid) {
    return syscall(Sys::eq_revoke, static_cast<u64>(qid), pid);
}

u32 eq_drive(long qid, void* base, void (*cb)(const EqEvent&)) {
    u8* b = static_cast<u8*>(base);
    volatile u8* h = b;

    u32 chunk_size = ld32(h + 4);
    u32 nchunks = ld32(h + 8);
    u64 cons = ld64(h + 24);
    u32 processed = 0;
    u32 idx = static_cast<u32>(cons % nchunks);

    for (;;) {
        volatile u8* ch = chunk(b, chunk_size, idx);
        u32 progress = ld32(ch + 4);
        u32 valid = progress & ~EQ_FULL;
        u32 off = static_cast<u32>(ld64(h + 40));
        bool closed = (progress & EQ_FULL) != 0;

        while (off + EQ_ELEM_HDR <= valid) {
            u32 len = ld32(ch + 8 + off);
            if (len < EQ_ELEM_HDR || off + len > valid) {
                break; // malformed tail, wait for more
            }
            EqEvent ev;
            ev.opcode = ld32(ch + 8 + off + 4);
            ev.sender = ld64(ch + 8 + off + 8);
            ev.payload = const_cast<const u8*>(ch + 8 + off + EQ_ELEM_HDR);
            ev.len = len - EQ_ELEM_HDR;
            cb(ev);
            ++processed;
            off += len;
        }

        if (closed && off == valid) {
            // chunk fully consumed: return it to the pool, release credit,
            // then follow the producer's chain to the next chunk
            u32 next = ld32(ch + 0);
            ++cons;
            st64(h + 24, cons);
            st64(h + 40, 0);
            st32(ch + 4, 0);
            eq_kick(qid);
            if (next >= nchunks) {
                break; // broken chain: stop, the kernel flags the channel
            }
            idx = next;
            continue;
        }
        // open chunk with no new bytes: remember the read position and let
        // the caller sleep on the doorbell
        st64(h + 40, off);
        break;
    }
    return processed;
}

u32 eq_errors(void* base, u32* code) {
    volatile u8* h = static_cast<volatile u8*>(base);
    u32 notify = ld32(h + 12);
    if ((notify & EQ_NOTIFY_ERROR) == 0) {
        return ld32(h + 36);
    }
    if (code != nullptr) {
        *code = ld32(h + 32);
    }
    // consumer clears the bit after reading; the count keeps history
    st32(h + 12, notify & ~EQ_NOTIFY_ERROR);
    return ld32(h + 36);
}

}
