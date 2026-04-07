cmake_minimum_required(VERSION 3.21)
project(y2skk-qt6-plugin VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

# ── Dependencies ──────────────────────────────────────────────────────────────

find_package(Qt6 REQUIRED COMPONENTS Gui)
find_package(Qt6 REQUIRED COMPONENTS GuiPrivate)

# Corrosion: integrates Cargo builds into CMake.
# Try a system installation first; fall back to FetchContent.
find_package(Corrosion QUIET)
if(NOT Corrosion_FOUND)
    include(FetchContent)
    FetchContent_Declare(
        Corrosion
        GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
        GIT_TAG        v0.5.1
    )
    FetchContent_MakeAvailable(Corrosion)
endif()

# ── Rust staticlib (adapter-qt6) ──────────────────────────────────────────────

# Import the Rust crate.  Corrosion will run `cargo build` and expose the
# resulting staticlib as a CMake target named `adapter-qt6`.
corrosion_import_crate(
    MANIFEST_PATH "${CMAKE_SOURCE_DIR}/../../Cargo.toml"
    CRATES adapter-qt6
)

# ── Qt6 plugin (C++ shim) ─────────────────────────────────────────────────────

qt_add_plugin(y2skk-qt6-plugin
    CLASS_NAME Y2skkInputContextPlugin
    PLUGIN_TYPE platforminputcontexts
)

target_sources(y2skk-qt6-plugin PRIVATE
    src/plugin.cpp
    src/context.cpp
    src/y2skk.json
)

target_include_directories(y2skk-qt6-plugin PRIVATE
    include
    src
)

target_link_libraries(y2skk-qt6-plugin PRIVATE
    Qt6::Gui
    Qt6::GuiPrivate
    # Corrosion copies the staticlib as libadapter_qt6.a (underscores) but
    # generates a -ladapter-qt6 (hyphens) linker flag which the linker cannot
    # resolve.  Work around by linking the file directly and declaring the
    # Cargo build target as an explicit dependency.
    "${CMAKE_BINARY_DIR}/libadapter_qt6.a"
)
add_dependencies(y2skk-qt6-plugin cargo-build_adapter_qt6)

# ── Install ───────────────────────────────────────────────────────────────────

# Query the absolute plugin directory from qmake so the install destination
# is independent of CMAKE_INSTALL_PREFIX.
get_target_property(_qmake Qt6::qmake IMPORTED_LOCATION)
execute_process(
    COMMAND "${_qmake}" -query QT_INSTALL_PLUGINS
    OUTPUT_VARIABLE _qt6_plugin_dir
    OUTPUT_STRIP_TRAILING_WHITESPACE
)
install(TARGETS y2skk-qt6-plugin
    LIBRARY DESTINATION "${_qt6_plugin_dir}/platforminputcontexts"
)
