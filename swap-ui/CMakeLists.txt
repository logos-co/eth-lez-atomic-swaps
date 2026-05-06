cmake_minimum_required(VERSION 3.14)
project(SwapUiPlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. Set LOGOS_MODULE_BUILDER_ROOT.")
endif()

logos_module(
    NAME swap_ui
    REP_FILE src/swap_ui.rep
    SOURCES
        src/swap_ui_interface.h
        src/swap_ui_plugin.h
        src/swap_ui_plugin.cpp
)
