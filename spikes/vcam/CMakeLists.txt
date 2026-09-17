cmake_minimum_required(VERSION 3.25)
project(chromafree_vcam_spike LANGUAGES CXX)

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

set(SPIKE_COMPILE_OPTIONS /W4 /WX /permissive- /EHsc /utf-8 /external:anglebrackets /external:W0)
set(SPIKE_DEFINITIONS UNICODE _UNICODE)

add_library(vcam-spike SHARED
    source/dllmain.cpp
    source/frame_source.cpp
    source/log.cpp
    source/media_source.cpp
    source/media_stream.cpp
    source/vcam-spike.def
)
target_precompile_headers(vcam-spike PRIVATE source/pch.h)
target_compile_options(vcam-spike PRIVATE ${SPIKE_COMPILE_OPTIONS})
target_compile_definitions(vcam-spike PRIVATE ${SPIKE_DEFINITIONS})
target_link_libraries(vcam-spike PRIVATE WIL::WIL mfplat mfuuid mfsensorgroup wtsapi32 advapi32 propsys windowsapp)

add_executable(vcam-spike-register register/main.cpp)
target_compile_options(vcam-spike-register PRIVATE ${SPIKE_COMPILE_OPTIONS})
target_compile_definitions(vcam-spike-register PRIVATE ${SPIKE_DEFINITIONS})
target_link_libraries(vcam-spike-register PRIVATE WIL::WIL mfplat mfsensorgroup ole32)

add_executable(vcam-spike-smoke smoke/main.cpp)
target_compile_options(vcam-spike-smoke PRIVATE ${SPIKE_COMPILE_OPTIONS})
target_compile_definitions(vcam-spike-smoke PRIVATE ${SPIKE_DEFINITIONS})
target_link_libraries(vcam-spike-smoke PRIVATE WIL::WIL mfplat mfuuid ole32)
