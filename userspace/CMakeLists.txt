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
    -fno-asynchronous-unwind-tables
    -fno-pie
    -Wall
    -Wextra
    -Werror
)

add_library(shz STATIC src/syscall.cpp src/io.cpp src/eq.cpp)
target_include_directories(shz PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)

add_executable(init programs/init/main.cpp src/start.cpp)
target_link_libraries(init PRIVATE shz)
target_link_options(init PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)

add_executable(hello programs/hello/main.cpp src/start.cpp)
target_link_libraries(hello PRIVATE shz)
target_link_options(hello PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)

add_executable(fsd programs/fsd/main.cpp src/start.cpp)
target_link_libraries(fsd PRIVATE shz)
target_link_options(fsd PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)

add_executable(rtcd programs/rtcd/main.cpp src/start.cpp)
target_link_libraries(rtcd PRIVATE shz)
target_link_options(rtcd PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)

add_executable(consoled programs/consoled/main.cpp src/start.cpp)
target_link_libraries(consoled PRIVATE shz)
target_link_options(consoled PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(gdemo programs/gdemo/main.cpp src/start.cpp)
target_link_libraries(gdemo PRIVATE shz)
target_link_options(gdemo PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(gdemo64 programs/gdemo64/main.cpp src/start.cpp)
target_link_libraries(gdemo64 PRIVATE shz)
target_link_options(gdemo64 PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(devd programs/devd/main.cpp src/start.cpp)
target_link_libraries(devd PRIVATE shz)
target_link_options(devd PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(capsdemo programs/capsdemo/main.cpp src/start.cpp)
target_link_libraries(capsdemo PRIVATE shz)
target_link_options(capsdemo PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(capsuser programs/capsuser/main.cpp src/start.cpp)
target_link_libraries(capsuser PRIVATE shz)
target_link_options(capsuser PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(timed programs/timed/main.cpp src/start.cpp)
target_link_libraries(timed PRIVATE shz)
target_link_options(timed PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(ps programs/ps/main.cpp src/start.cpp)
target_link_libraries(ps PRIVATE shz)
target_link_options(ps PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(kill programs/kill/main.cpp src/start.cpp)
target_link_libraries(kill PRIVATE shz)
target_link_options(kill PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(sleeper programs/sleeper/main.cpp src/start.cpp)
target_link_libraries(sleeper PRIVATE shz)
target_link_options(sleeper PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(fuxdemo programs/fuxdemo/main.cpp src/start.cpp)
target_link_libraries(fuxdemo PRIVATE shz)
target_link_options(fuxdemo PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(uardemo programs/uardemo/main.cpp src/start.cpp)
target_link_libraries(uardemo PRIVATE shz)
target_link_options(uardemo PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(shmdemo programs/shmdemo/main.cpp src/start.cpp)
target_link_libraries(shmdemo PRIVATE shz)
target_link_options(shmdemo PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
add_executable(shmpeer programs/shmpeer/main.cpp src/start.cpp)
target_link_libraries(shmpeer PRIVATE shz)
target_link_options(shmpeer PRIVATE -nostdlib -static -no-pie -Wl,--build-id=none -Wl,-z,noexecstack -Wl,-T,${CMAKE_CURRENT_SOURCE_DIR}/link.ld)
