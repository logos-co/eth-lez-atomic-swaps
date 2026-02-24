cmake_minimum_required(VERSION 3.21)
project(atomic-swaps-ui LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 6.5 REQUIRED COMPONENTS Quick QuickControls2 Concurrent)

qt_standard_project_setup(REQUIRES 6.5)

# --- Rust FFI library ---
set(FFI_DIR "${CMAKE_SOURCE_DIR}/../swap-ffi")
set(FFI_INCLUDE "${FFI_DIR}/include")

# Use release build if available, fall back to debug.
if(EXISTS "${FFI_DIR}/target/release/libswap_ffi.dylib")
    set(FFI_LIB "${FFI_DIR}/target/release/libswap_ffi.dylib")
elseif(EXISTS "${FFI_DIR}/target/debug/libswap_ffi.dylib")
    set(FFI_LIB "${FFI_DIR}/target/debug/libswap_ffi.dylib")
else()
    message(FATAL_ERROR "libswap_ffi.dylib not found. Run 'cd swap-ffi && cargo build' first.")
endif()

add_library(swap_ffi SHARED IMPORTED)
set_target_properties(swap_ffi PROPERTIES
    IMPORTED_LOCATION "${FFI_LIB}"
    INTERFACE_INCLUDE_DIRECTORIES "${FFI_INCLUDE}"
)

# --- Qt app ---
qt_add_executable(atomic-swaps-ui
    src/main.cpp
    src/swapbackend.h
    src/swapbackend.cpp
)

# Flatten qml/ subdir paths into the module root so the QML engine
# resolves types (especially the Theme singleton) correctly (QTP0004).
set_source_files_properties(qml/Theme.qml        PROPERTIES QT_QML_SINGLETON_TYPE TRUE QT_RESOURCE_ALIAS Theme.qml)
set_source_files_properties(qml/Main.qml          PROPERTIES QT_RESOURCE_ALIAS Main.qml)
set_source_files_properties(qml/ConfigPanel.qml   PROPERTIES QT_RESOURCE_ALIAS ConfigPanel.qml)
set_source_files_properties(qml/MakerView.qml     PROPERTIES QT_RESOURCE_ALIAS MakerView.qml)
set_source_files_properties(qml/TakerView.qml     PROPERTIES QT_RESOURCE_ALIAS TakerView.qml)
set_source_files_properties(qml/RefundView.qml    PROPERTIES QT_RESOURCE_ALIAS RefundView.qml)
set_source_files_properties(qml/ProgressStepper.qml PROPERTIES QT_RESOURCE_ALIAS ProgressStepper.qml)
set_source_files_properties(qml/ResultCard.qml    PROPERTIES QT_RESOURCE_ALIAS ResultCard.qml)

qt_add_qml_module(atomic-swaps-ui
    URI AtomicSwaps
    VERSION 1.0
    QML_FILES
        qml/Main.qml
        qml/Theme.qml
        qml/ConfigPanel.qml
        qml/MakerView.qml
        qml/TakerView.qml
        qml/RefundView.qml
        qml/ProgressStepper.qml
        qml/ResultCard.qml
)

target_link_libraries(atomic-swaps-ui PRIVATE
    Qt6::Quick
    Qt6::QuickControls2
    Qt6::Concurrent
    swap_ffi
)

# Copy dylib next to executable so RPATH finds it.
add_custom_command(TARGET atomic-swaps-ui POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${FFI_LIB}" "$<TARGET_FILE_DIR:atomic-swaps-ui>"
)

# macOS RPATH
set_target_properties(atomic-swaps-ui PROPERTIES
    MACOSX_BUNDLE TRUE
    INSTALL_RPATH "@executable_path"
    BUILD_WITH_INSTALL_RPATH TRUE
)
