cmake_minimum_required(VERSION 3.21)
project(y2skk-settings-qt6 VERSION 0.9.5 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

# ── Dependencies ──────────────────────────────────────────────────────────────

find_package(Qt6 REQUIRED COMPONENTS Widgets Concurrent)
find_package(Threads REQUIRED)

# Corrosion integrates the Cargo build into CMake.  Prefer a system install.
# Network fetching is opt-in and OFF by default so plain `cmake` builds are
# reproducible / offline-safe; `cargo xtask install --gui` passes
# -DY2SKK_FETCH_CORROSION=ON explicitly for developer convenience.
option(Y2SKK_FETCH_CORROSION "Fetch Corrosion via FetchContent when not installed" OFF)
find_package(Corrosion QUIET)
if(NOT Corrosion_FOUND)
    if(Y2SKK_FETCH_CORROSION)
        include(FetchContent)
        FetchContent_Declare(
            Corrosion
            GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
            GIT_TAG        v0.5.1
        )
        FetchContent_MakeAvailable(Corrosion)
    else()
        message(FATAL_ERROR
            "Corrosion not found. Install it (https://github.com/corrosion-rs/corrosion) "
            "or re-run with -DY2SKK_FETCH_CORROSION=ON to fetch it automatically.")
    endif()
endif()

# ── Rust staticlib (y2skk-settings) ────────────────────────────────────────────

corrosion_import_crate(
    MANIFEST_PATH "${CMAKE_SOURCE_DIR}/../../Cargo.toml"
    CRATES y2skk-settings
)

# ── Qt Widgets executable ───────────────────────────────────────────────────────

# The CMake target name must differ from the Corrosion crate target
# (`y2skk-settings`); the installed binary is named y2skk-settings-qt6 (leaving
# room for a future GTK build, e.g. y2skk-settings-gtk) via OUTPUT_NAME.
add_executable(y2skk_settings_app
    src/main.cpp
    src/settings_window.cpp
)
set_target_properties(y2skk_settings_app PROPERTIES OUTPUT_NAME y2skk-settings-qt6)

target_include_directories(y2skk_settings_app PRIVATE include src)

target_link_libraries(y2skk_settings_app PRIVATE
    Qt6::Widgets
    Qt6::Concurrent
    # Corrosion copies the staticlib as liby2skk_settings.a (underscores) but
    # generates a -ly2skk-settings (hyphens) linker flag the linker cannot
    # resolve.  Link the file directly and declare the Cargo build as a
    # dependency (same workaround as shim/qt6).
    "${CMAKE_BINARY_DIR}/liby2skk_settings.a"
    # System libraries the Rust staticlib (std + zbus) needs.
    Threads::Threads
    ${CMAKE_DL_LIBS}
)
add_dependencies(y2skk_settings_app cargo-build_y2skk_settings)

# ── Install ───────────────────────────────────────────────────────────────────

include(GNUInstallDirs)
install(TARGETS y2skk_settings_app RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
