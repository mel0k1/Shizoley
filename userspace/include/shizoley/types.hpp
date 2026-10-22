#pragma once

namespace shz {

using u8 = unsigned char;
using u16 = unsigned short;
using u32 = unsigned int;
using u64 = unsigned long long;
using i8 = signed char;
using i16 = short;
using i32 = int;
using i64 = long long;
using usize = u64;
using isize = i64;

// the kernel maps the whole initrd read-only here in every process
constexpr u64 INITRD_BASE = 0x20000000ull;

// guest ram window installed by GUEST_CREATE (pages*4KiB starting here)
constexpr u64 GUEST_WINDOW = 0x30000000ull;

}
