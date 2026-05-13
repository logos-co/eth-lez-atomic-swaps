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
        src/swap_delivery_adapter.h
        src/swap_delivery_adapter.cpp
        generated_code/swap_qt_glue.h
        generated_code/swap_dispatch.cpp
    INCLUDE_DIRS
        ${CMAKE_CURRENT_SOURCE_DIR}/generated_code
        ${CMAKE_CURRENT_SOURCE_DIR}/lib
)

set(SWAP_FFI_LIB_DIR "${CMAKE_CURRENT_SOURCE_DIR}/lib")

if(WIN32)
    find_library(SWAP_FFI_IMPORT_LIBRARY
        NAMES swap_ffi libswap_ffi
        PATHS "${SWAP_FFI_LIB_DIR}"
        NO_DEFAULT_PATH
    )
    find_file(SWAP_FFI_RUNTIME_LIBRARY
        NAMES swap_ffi.dll libswap_ffi.dll
        PATHS "${SWAP_FFI_LIB_DIR}"
        NO_DEFAULT_PATH
    )

    if(NOT SWAP_FFI_IMPORT_LIBRARY OR NOT SWAP_FFI_RUNTIME_LIBRARY)
        message(FATAL_ERROR "swap_ffi is missing; ensure the Nix external library build staged both the import library and DLL into swap-module/lib")
    endif()

    target_link_libraries(swap_module_plugin PRIVATE "${SWAP_FFI_IMPORT_LIBRARY}")
    install(FILES "${SWAP_FFI_RUNTIME_LIBRARY}" DESTINATION lib)
else()
    find_library(SWAP_FFI_LIBRARY
        NAMES swap_ffi libswap_ffi
        PATHS "${SWAP_FFI_LIB_DIR}"
        NO_DEFAULT_PATH
    )

    if(NOT SWAP_FFI_LIBRARY)
        message(FATAL_ERROR "swap_ffi is missing; ensure the Nix external library build staged it into swap-module/lib")
    endif()

    target_link_libraries(swap_module_plugin PRIVATE "${SWAP_FFI_LIBRARY}")
    install(FILES "${SWAP_FFI_LIBRARY}" DESTINATION lib)
endif()
