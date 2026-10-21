#include <shizoley/io.hpp>

namespace shz {

int main() {
    println("shizoley 0.1");
    println("paparaci says: goblin online");
    u64 n = 1;
    for (int i = 0; i < 10; ++i) {
        n *= 3;
    }
    println("3^10 =", n);
    return 0;
}

}
