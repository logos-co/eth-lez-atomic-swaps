cmake_minimum_required(VERSION 3.21)
project(lez-atomic-swap-module LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

# macOS: ensure SDK is set so pthread detection succeeds
if(APPLE AND NOT CMAKE_OSX_SYSROOT)
    execute_process(
        COMMAND xcrun --show-sdk-path
        OUTPUT_VARIABLE _sdk_path
        OUTPUT_STRIP_TRAILING_WHITESPACE
    )
    set(CMAKE_OSX_SYSROOT "${_sdk_path}" CACHE PATH "macOS SDK")
endif()

option(BUILD_PLUGIN     "Build as Logos Core module plugin" OFF)
option(BUILD_APP_PLUGIN "Build as logos-app IComponent UI plugin" OFF)

find_package(Qt6 6.5 REQUIRED COMPONENTS Quick QuickControls2 Concurrent)

if(BUILD_APP_PLUGIN)
    find_package(Qt6 6.5 REQUIRED COMPONENTS Widgets QuickWidgets)
endif()

qt_standard_project_setup(REQUIRES 6.5)

# --- Rust FFI library ---
set(FFI_DIR "${CMAKE_SOURCE_DIR}/../swap-ffi")
set(FFI_INCLUDE "${FFI_DIR}/include")

# Platform-specific shared library name
if(APPLE)
    set(FFI_LIB_NAME "libswap_ffi.dylib")
elseif(WIN32)
    set(FFI_LIB_NAME "swap_ffi.dll")
else()
    set(FFI_LIB_NAME "libswap_ffi.so")
endif()

# Use release build if available, fall back to debug.
if(EXISTS "${FFI_DIR}/target/release/${FFI_LIB_NAME}")
    set(FFI_LIB "${FFI_DIR}/target/release/${FFI_LIB_NAME}")
elseif(EXISTS "${FFI_DIR}/target/debug/${FFI_LIB_NAME}")
    set(FFI_LIB "${FFI_DIR}/target/debug/${FFI_LIB_NAME}")
else()
    message(FATAL_ERROR "${FFI_LIB_NAME} not found. Run 'cd swap-ffi && cargo build' first.")
endif()

add_library(swap_ffi SHARED IMPORTED)
set_target_properties(swap_ffi PROPERTIES
    IMPORTED_LOCATION "${FFI_LIB}"
    INTERFACE_INCLUDE_DIRECTORIES "${FFI_INCLUDE}"
)

# --- Sources shared between all build modes ---
set(SHARED_SOURCES
    src/lez_atomic_swap_module.h
    src/lez_atomic_swap_module.cpp
    src/swap_backend.h
    src/swap_backend.cpp
)

# --- Build mode ---
if(BUILD_APP_PLUGIN)
    # ── logos-app IComponent UI plugin ──
    # Produces a shared library that logos-app loads as a UI plugin.
    # Requires logos-cpp-sdk headers and logos-app IComponent.h.
    qt_add_library(lez_atomic_swap_module SHARED
        ${SHARED_SOURCES}
        src/icomponent_adapter.cpp
    )
    target_compile_definitions(lez_atomic_swap_module PRIVATE LOGOS_APP_PLUGIN)

    # logos-app interface header (IComponent.h)
    if(NOT LOGOS_APP_INTERFACES_DIR)
        set(LOGOS_APP_INTERFACES_DIR "" CACHE PATH "Path to logos-app/app/interfaces/")
    endif()

    if(LOGOS_APP_INTERFACES_DIR)
        target_include_directories(lez_atomic_swap_module PRIVATE ${LOGOS_APP_INTERFACES_DIR})
    endif()

    # logos-cpp-sdk headers (LogosAPI, LogosAPIClient) for delivery module communication
    if(NOT LOGOS_CPP_SDK_DIR)
        set(LOGOS_CPP_SDK_DIR "" CACHE PATH "Path to logos-cpp-sdk/cpp/")
    endif()

    if(LOGOS_CPP_SDK_DIR)
        target_include_directories(lez_atomic_swap_module PRIVATE ${LOGOS_CPP_SDK_DIR})
    endif()

    target_link_libraries(lez_atomic_swap_module PRIVATE
        Qt6::Widgets
        Qt6::QuickWidgets
    )

    # Output: logos-app expects <name>/<name>.dylib in the plugins dir.
    # Set OUTPUT_NAME so we get lez_atomic_swap.dylib (not liblez_atomic_swap_module.dylib).
    set_target_properties(lez_atomic_swap_module PROPERTIES
        OUTPUT_NAME "lez_atomic_swap"
        PREFIX ""
    )

    # RPATH: find libswap_ffi.dylib next to the plugin at runtime
    if(APPLE)
        set_target_properties(lez_atomic_swap_module PROPERTIES
            INSTALL_RPATH "@loader_path"
            BUILD_WITH_INSTALL_RPATH TRUE
        )
    else()
        set_target_properties(lez_atomic_swap_module PROPERTIES
            INSTALL_RPATH "$ORIGIN"
            BUILD_WITH_INSTALL_RPATH TRUE
        )
    endif()

elseif(BUILD_PLUGIN)
    # ── Logos Core module plugin ──
    qt_add_plugin(lez_atomic_swap_module
        ${SHARED_SOURCES}
        src/plugin.cpp
    )
    target_compile_definitions(lez_atomic_swap_module PRIVATE LOGOS_CORE_PLUGIN)

else()
    # ── Standalone executable ──
    qt_add_executable(lez_atomic_swap_module
        ${SHARED_SOURCES}
        src/main.cpp
    )

    if(APPLE)
        set_target_properties(lez_atomic_swap_module PROPERTIES
            MACOSX_BUNDLE TRUE
            INSTALL_RPATH "@executable_path"
            BUILD_WITH_INSTALL_RPATH TRUE
        )
    else()
        set_target_properties(lez_atomic_swap_module PROPERTIES
            INSTALL_RPATH "$ORIGIN"
            BUILD_WITH_INSTALL_RPATH TRUE
        )
    endif()

    # Copy dylib next to executable so RPATH finds it.
    add_custom_command(TARGET lez_atomic_swap_module POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different "${FFI_LIB}" "$<TARGET_FILE_DIR:lez_atomic_swap_module>"
    )
endif()

# --- Include paths ---
target_include_directories(lez_atomic_swap_module PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

# --- QML module (shared between all modes) ---
# Flatten qml/ subdir paths into the module root so the QML engine
# resolves types (especially the Theme singleton) correctly (QTP0004).
set_source_files_properties(qml/Theme.qml             PROPERTIES QT_QML_SINGLETON_TYPE TRUE QT_RESOURCE_ALIAS Theme.qml)
set_source_files_properties(qml/Main.qml              PROPERTIES QT_RESOURCE_ALIAS Main.qml)
set_source_files_properties(qml/AtomicSwapView.qml    PROPERTIES QT_RESOURCE_ALIAS AtomicSwapView.qml)
set_source_files_properties(qml/ConfigPanel.qml       PROPERTIES QT_RESOURCE_ALIAS ConfigPanel.qml)
set_source_files_properties(qml/MakerView.qml         PROPERTIES QT_RESOURCE_ALIAS MakerView.qml)
set_source_files_properties(qml/TakerView.qml         PROPERTIES QT_RESOURCE_ALIAS TakerView.qml)
set_source_files_properties(qml/RefundView.qml        PROPERTIES QT_RESOURCE_ALIAS RefundView.qml)
set_source_files_properties(qml/ProgressStepper.qml   PROPERTIES QT_RESOURCE_ALIAS ProgressStepper.qml)
set_source_files_properties(qml/ResultCard.qml        PROPERTIES QT_RESOURCE_ALIAS ResultCard.qml)

qt_add_qml_module(lez_atomic_swap_module
    URI LezAtomicSwap
    VERSION 1.0
    QML_FILES
        qml/Main.qml
        qml/Theme.qml
        qml/AtomicSwapView.qml
        qml/ConfigPanel.qml
        qml/MakerView.qml
        qml/TakerView.qml
        qml/RefundView.qml
        qml/ProgressStepper.qml
        qml/ResultCard.qml
)

# --- Link libraries ---
target_link_libraries(lez_atomic_swap_module PRIVATE
    Qt6::Quick
    Qt6::QuickControls2
    Qt6::Concurrent
    swap_ffi
)
