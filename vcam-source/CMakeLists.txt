cmake_minimum_required(VERSION 3.25)
project(chromafree_vcam_source LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")

include(FetchContent)
set(WIL_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(WIL_BUILD_PACKAGING OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
    wil
    GIT_REPOSITORY https://github.com/microsoft/wil.git
    GIT_TAG v1.0.260126.7
    GIT_SHALLOW TRUE
    SYSTEM
)
FetchContent_MakeAvailable(wil)

file(STRINGS ${CMAKE_CURRENT_SOURCE_DIR}/../Cargo.toml CHROMAFREE_VERSION_LINE REGEX "^version = \"[0-9]+\\.[0-9]+\\.[0-9]+\"")
string(REGEX REPLACE "^version = \"([0-9]+)\\.([0-9]+)\\.([0-9]+)\"" "\\1.\\2.\\3" CHROMAFREE_VERSION "${CHROMAFREE_VERSION_LINE}")
string(REPLACE "." ";" CHROMAFREE_VERSION_PARTS "${CHROMAFREE_VERSION}")
list(GET CHROMAFREE_VERSION_PARTS 0 CHROMAFREE_VERSION_MAJOR)
list(GET CHROMAFREE_VERSION_PARTS 1 CHROMAFREE_VERSION_MINOR)
list(GET CHROMAFREE_VERSION_PARTS 2 CHROMAFREE_VERSION_PATCH)
file(STRINGS ${CMAKE_CURRENT_SOURCE_DIR}/../Cargo.toml CHROMAFREE_AUTHORS_LINE REGEX "^authors = ")
string(REGEX REPLACE "^authors = \\[\"([^\"]+)\".*" "\\1" CHROMAFREE_AUTHOR "${CHROMAFREE_AUTHORS_LINE}")
configure_file(src/vcam-source.rc.in ${CMAKE_CURRENT_BINARY_DIR}/vcam-source.rc @ONLY)
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/../Cargo.toml)

set(CHROMAFREE_COMPILE_OPTIONS /W4 /WX /permissive- /EHsc /utf-8 /external:anglebrackets /external:W0)
set(CHROMAFREE_DEFINITIONS UNICODE _UNICODE)

add_library(vcam-source SHARED
    src/dllmain.cpp
    src/frame_channel.cpp
    src/media_source.cpp
    src/media_stream.cpp
    src/offline_frame.cpp
    src/log.cpp
    src/vcam-source.def
    ${CMAKE_CURRENT_BINARY_DIR}/vcam-source.rc
)
target_precompile_headers(vcam-source PRIVATE src/pch.h)
target_include_directories(vcam-source SYSTEM PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../crates/chromafree-ipc/include)
target_compile_options(vcam-source PRIVATE ${CHROMAFREE_COMPILE_OPTIONS})
target_compile_definitions(vcam-source PRIVATE ${CHROMAFREE_DEFINITIONS})
target_link_libraries(vcam-source PRIVATE WIL::WIL mfplat mfuuid mfsensorgroup wtsapi32 propsys windowscodecs windowsapp)

add_executable(vcam-source-smoke smoke/main.cpp)
target_compile_options(vcam-source-smoke PRIVATE ${CHROMAFREE_COMPILE_OPTIONS})
target_compile_definitions(vcam-source-smoke PRIVATE ${CHROMAFREE_DEFINITIONS})
target_link_libraries(vcam-source-smoke PRIVATE WIL::WIL mfplat mfuuid ole32)
