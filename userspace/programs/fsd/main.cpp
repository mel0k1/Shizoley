// fsd — file system service in ring 3: walks the initrd mapping
// (read-only at INITRD_BASE, size via syscall) and serves file contents
// to clients over the mailbox ipc. every request message is a file name.
#include <shizoley/io.hpp>
#include <shizoley/syscall.hpp>
#include <shizoley/types.hpp>

using namespace shz;

namespace {

// ustar walker over the shared initrd mapping
const u8* lookup(const char* name, u64* out_len) {
#ifdef FSD_NO_WALK
    (void)name;
    *out_len = 0;
    return nullptr;
#endif
    const volatile u8* img = reinterpret_cast<const volatile u8*>(INITRD_BASE);
    u64 size = static_cast<u64>(syscall(Sys::initrd_size));
    u64 off = 0;
    while (off + 512 <= size) {
        if (img[off] == 0) {
            break; // archive tail
        }
        char namebuf[101];
        u32 nl = 0;
        while (nl < 100 && img[off + nl] != 0) {
            namebuf[nl] = static_cast<char>(img[off + nl]);
            ++nl;
        }
        namebuf[nl] = '\0';

        u64 fsize = 0;
        for (int i = 0; i < 11; ++i) {
            u8 c = img[off + 124 + i];
            if (c >= '0' && c <= '7') {
                fsize = fsize * 8 + (c - '0');
            }
        }
        u64 data = off + 512;
        if (data + fsize > size) {
            break;
        }
        u32 i = 0;
        while (name[i] != '\0' && i < nl && namebuf[i] == name[i]) {
            ++i;
        }
        if (name[i] == '\0' && i == nl) {
            *out_len = fsize;
            return const_cast<const u8*>(
                reinterpret_cast<const volatile u8*>(INITRD_BASE + data));
        }
        off = data + (fsize + 511) / 512 * 512;
    }
    return nullptr;
}

}

namespace shz {

int main() {
    println("[fsd] pid ", static_cast<u64>(getpid()), " serving initrd at 0x20000000");
    char req[256];
    char reply[1024];
    for (;;) {
        for (u64 i = 0; i < sizeof(req); ++i) {
            req[i] = 0;
        }
        long from = recv(0, req, sizeof(req));
        if (from <= 0) {
            continue;
        }
        u64 len = 0;
        const u8* content = lookup(req, &len);
        if (content != nullptr && len < sizeof(reply)) {
            for (u64 i = 0; i < len; ++i) {
                reply[i] = static_cast<char>(content[i]);
            }
            send(static_cast<u64>(from), reply, static_cast<u64>(len));
            println("[fsd] served '", req, "' (", static_cast<u64>(len),
                    " bytes) to ", static_cast<u64>(from));
        } else {
            const char* err = "ERR:not found";
            u64 n = 0;
            while (err[n] != '\0') {
                ++n;
            }
            send(static_cast<u64>(from), err, n);
            println("[fsd] miss '", req, "'");
        }
    }
}

}
