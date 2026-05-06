cmake_minimum_required(VERSION 3.14)
project(LogosSwapPlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. Set LOGOS_MODULE_BUILDER_ROOT.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(
    NAME swap
    EXTERNAL_LIBS
        swap_ffi
    SOURCES
        src/swap_impl.h
        src/swap_impl.cpp
        generated_code/swap_qt_glue.h
        generated_code/swap_dispatch.cpp
    INCLUDE_DIRS
        ${CMAKE_CURRENT_SOURCE_DIR}/generated_code
        ${CMAKE_CURRENT_SOURCE_DIR}/lib
)
