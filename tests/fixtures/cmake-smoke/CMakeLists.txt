cmake_minimum_required(VERSION 3.20)
project(knight_cmake_smoke C)

add_library(answer STATIC answer.c)
add_executable(smoke main.c)
target_include_directories(smoke PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(smoke PRIVATE answer)
