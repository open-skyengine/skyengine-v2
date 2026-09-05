cmake_minimum_required(VERSION 3.18)
project(skyengine_ffi LANGUAGES C)

option(SKYENGINE_BUILD_SHARED_ONLY "Build only the Flutter shared library" ON)

find_program(
  CARGO_EXECUTABLE cargo
  HINTS
    "$ENV{USERPROFILE}/.cargo/bin"
    "$ENV{HOME}/.cargo/bin"
  REQUIRED
)

set(SKYENGINE_RUST_TARGET "")
set(SKYENGINE_CARGO_ENV
  "CARGO_TARGET_DIR=${CMAKE_CURRENT_BINARY_DIR}/cargo-target"
)

if(ANDROID)
  if(CMAKE_ANDROID_ARCH_ABI STREQUAL "arm64-v8a")
    set(SKYENGINE_RUST_TARGET "aarch64-linux-android")
    set(SKYENGINE_ANDROID_CLANG_PREFIX "aarch64-linux-android")
  elseif(CMAKE_ANDROID_ARCH_ABI STREQUAL "armeabi-v7a")
    set(SKYENGINE_RUST_TARGET "armv7-linux-androideabi")
    set(SKYENGINE_ANDROID_CLANG_PREFIX "armv7a-linux-androideabi")
  elseif(CMAKE_ANDROID_ARCH_ABI STREQUAL "x86_64")
    set(SKYENGINE_RUST_TARGET "x86_64-linux-android")
    set(SKYENGINE_ANDROID_CLANG_PREFIX "x86_64-linux-android")
  elseif(CMAKE_ANDROID_ARCH_ABI STREQUAL "x86")
    set(SKYENGINE_RUST_TARGET "i686-linux-android")
    set(SKYENGINE_ANDROID_CLANG_PREFIX "i686-linux-android")
  else()
    message(FATAL_ERROR "Unsupported Android ABI: ${CMAKE_ANDROID_ARCH_ABI}")
  endif()

  if(CMAKE_HOST_SYSTEM_NAME STREQUAL "Windows")
    set(SKYENGINE_NDK_HOST "windows-x86_64")
    set(SKYENGINE_CLANG_SUFFIX ".cmd")
  elseif(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin")
    set(SKYENGINE_NDK_HOST "darwin-x86_64")
    set(SKYENGINE_CLANG_SUFFIX "")
  else()
    set(SKYENGINE_NDK_HOST "linux-x86_64")
    set(SKYENGINE_CLANG_SUFFIX "")
  endif()

  if(DEFINED ANDROID_PLATFORM_LEVEL)
    set(SKYENGINE_ANDROID_API "${ANDROID_PLATFORM_LEVEL}")
  elseif(CMAKE_SYSTEM_VERSION MATCHES "^[0-9]+$")
    set(SKYENGINE_ANDROID_API "${CMAKE_SYSTEM_VERSION}")
  else()
    set(SKYENGINE_ANDROID_API "21")
  endif()

  string(TOUPPER "${SKYENGINE_RUST_TARGET}" SKYENGINE_RUST_TARGET_ENV)
  string(REPLACE "-" "_" SKYENGINE_RUST_TARGET_ENV "${SKYENGINE_RUST_TARGET_ENV}")
  set(SKYENGINE_ANDROID_LINKER
    "${CMAKE_ANDROID_NDK}/toolchains/llvm/prebuilt/${SKYENGINE_NDK_HOST}/bin/${SKYENGINE_ANDROID_CLANG_PREFIX}${SKYENGINE_ANDROID_API}-clang${SKYENGINE_CLANG_SUFFIX}"
  )
  if(NOT EXISTS "${SKYENGINE_ANDROID_LINKER}")
    message(FATAL_ERROR "Android NDK linker not found: ${SKYENGINE_ANDROID_LINKER}")
  endif()
  list(APPEND SKYENGINE_CARGO_ENV
    "CARGO_TARGET_${SKYENGINE_RUST_TARGET_ENV}_LINKER=${SKYENGINE_ANDROID_LINKER}"
  )
endif()

set(SKYENGINE_CARGO_ARGS
  build
  --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml"
  --package skyengine-ffi
  --release
  --locked
)
if(SKYENGINE_RUST_TARGET)
  list(APPEND SKYENGINE_CARGO_ARGS --target "${SKYENGINE_RUST_TARGET}")
  set(SKYENGINE_RUST_OUTPUT_DIR
    "${CMAKE_CURRENT_BINARY_DIR}/cargo-target/${SKYENGINE_RUST_TARGET}/release"
  )
else()
  set(SKYENGINE_RUST_OUTPUT_DIR
    "${CMAKE_CURRENT_BINARY_DIR}/cargo-target/release"
  )
endif()

if(WIN32 AND NOT ANDROID)
  set(SKYENGINE_RUST_STATIC "${SKYENGINE_RUST_OUTPUT_DIR}/skyengine.lib")
else()
  set(SKYENGINE_RUST_STATIC "${SKYENGINE_RUST_OUTPUT_DIR}/libskyengine.a")
endif()

file(GLOB_RECURSE SKYENGINE_RUST_SOURCES CONFIGURE_DEPENDS
  "${CMAKE_CURRENT_SOURCE_DIR}/crates/skyengine-arm/src/*.rs"
  "${CMAKE_CURRENT_SOURCE_DIR}/crates/skyengine-core/src/*.rs"
  "${CMAKE_CURRENT_SOURCE_DIR}/crates/skyengine-ffi/src/*.rs"
)

add_custom_command(
  OUTPUT "${SKYENGINE_RUST_STATIC}"
  COMMAND "${CMAKE_COMMAND}" -E env
    ${SKYENGINE_CARGO_ENV}
    "${CARGO_EXECUTABLE}" ${SKYENGINE_CARGO_ARGS}
  DEPENDS
    ${SKYENGINE_RUST_SOURCES}
    "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml"
    "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.lock"
    "${CMAKE_CURRENT_SOURCE_DIR}/crates/skyengine-arm/Cargo.toml"
    "${CMAKE_CURRENT_SOURCE_DIR}/crates/skyengine-core/Cargo.toml"
    "${CMAKE_CURRENT_SOURCE_DIR}/crates/skyengine-ffi/Cargo.toml"
  WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
  COMMENT "Building SkyEngine Rust bridge for ${CMAKE_ANDROID_ARCH_ABI}${SKYENGINE_RUST_TARGET}"
  VERBATIM
)
add_custom_target(skyengine-rust DEPENDS "${SKYENGINE_RUST_STATIC}")

add_library(skyengine-shared SHARED
  "${CMAKE_CURRENT_SOURCE_DIR}/cmake/skyengine_stub.c"
)
add_dependencies(skyengine-shared skyengine-rust)
set_property(TARGET skyengine-shared APPEND PROPERTY
  LINK_DEPENDS "${SKYENGINE_RUST_STATIC}"
)
set_target_properties(skyengine-shared PROPERTIES OUTPUT_NAME skyengine)

if(MSVC)
  target_sources(skyengine-shared PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/cmake/skyengine.def"
  )
  target_link_options(skyengine-shared PRIVATE
    "/WHOLEARCHIVE:${SKYENGINE_RUST_STATIC}"
  )
  target_link_libraries(skyengine-shared PRIVATE
    advapi32 bcrypt ntdll userenv ws2_32
  )
elseif(APPLE)
  target_link_options(skyengine-shared PRIVATE
    "LINKER:-force_load,${SKYENGINE_RUST_STATIC}"
  )
  target_link_libraries(skyengine-shared PRIVATE
    "-framework CoreFoundation"
    "-framework Security"
  )
else()
  target_link_options(skyengine-shared PRIVATE
    "LINKER:--whole-archive,${SKYENGINE_RUST_STATIC},--no-whole-archive"
  )
  if(ANDROID)
    target_link_libraries(skyengine-shared PRIVATE dl log m)
  else()
    find_package(Threads REQUIRED)
    target_link_libraries(skyengine-shared PRIVATE Threads::Threads dl m)
  endif()
endif()
