cmake_minimum_required(VERSION 3.16)

project(skyengine_v2 VERSION 0.1.0 LANGUAGES C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)

add_executable(skyengine
  src/main.c
)

if(MSVC)
  target_compile_options(skyengine PRIVATE /W4)
else()
  target_compile_options(skyengine PRIVATE -Wall -Wextra -Wpedantic)
endif()

include(CTest)
if(BUILD_TESTING)
  add_test(NAME hello-world COMMAND $<TARGET_FILE:skyengine>)
  set_tests_properties(hello-world PROPERTIES
    PASS_REGULAR_EXPRESSION "^helloworld"
  )
endif()
