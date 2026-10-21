cmake_minimum_required(VERSION 3.16)
project(shizoley_userspace CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_compile_options(
    -ffreestanding
    -fno-exceptions
    -fno-rtti
    -fno-stack-protector
    -fno-threadsafe-statics
    -Wall
    -Wextra
    -Werror
)

add_library(shz STATIC src/syscall.cpp src/io.cpp)
target_include_directories(shz PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)

add_executable(init programs/init/main.cpp src/start.cpp)
target_link_libraries(init PRIVATE shz)
target_link_options(init PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack)
