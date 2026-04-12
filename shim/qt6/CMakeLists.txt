cmake_minimum_required(VERSION 3.21)
project(y2skk-qt6-plugin VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

# ── Dependencies ──────────────────────────────────────────────────────────────

find_package(Qt6 REQUIRED COMPONENTS Gui)

# Add Qt6 private headers (for QPlatformInputContext / QPlatformInputContextPlugin)
# without calling find_package(GuiPrivate), which would emit a version-coupling
# warning.  Private headers live at <QtGui-include-dir>/<Qt-version>/QtGui.
# Locate Qt6 private headers (for QPlatformInputContext / QPlatformInputContextPlugin)
# without calling find_package(GuiPrivate), which emits a version-coupling warning.
# Qt6::GuiPrivate normally adds two paths per module:
#   <qt-include-dir>/<module>/<ver>  and  <qt-include-dir>/<module>/<ver>/<module>
# We replicate this for both QtGui and QtCore (GuiPrivate depends on CorePrivate).
function(collect_qt6_private_includes out_var module_include_dirs module_name)
    foreach(_dir IN LISTS ${module_include_dirs})
        if(EXISTS "${_dir}/${Qt6_VERSION}/${module_name}")
            set(_base "${_dir}/${Qt6_VERSION}")
            list(APPEND _result "${_base}" "${_base}/${module_name}")
            break()
        endif()
    endforeach()
    set(${out_var} "${_result}" PARENT_SCOPE)
endfunction()

collect_qt6_private_includes(_qt6_gui_private_incs  Qt6Gui_INCLUDE_DIRS  QtGui)
collect_qt6_private_includes(_qt6_core_private_incs Qt6Core_INCLUDE_DIRS QtCore)

if(NOT _qt6_gui_private_incs)
    message(FATAL_ERROR
        "Cannot find Qt6 Gui private headers under Qt6Gui_INCLUDE_DIRS"
        " (${Qt6Gui_INCLUDE_DIRS}) for version ${Qt6_VERSION}")
endif()

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
    src/candidate_window.cpp
    src/status_window.cpp
    src/y2skk.json
)

target_include_directories(y2skk-qt6-plugin PRIVATE
    include
    src
    ${_qt6_gui_private_incs}
    ${_qt6_core_private_incs}
)

target_link_libraries(y2skk-qt6-plugin PRIVATE
    Qt6::Gui
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

# Allow overriding the install directory (e.g. for packaging into a staging prefix).
set(Y2SKK_QT6_PLUGIN_DIR "" CACHE PATH
    "Override the Qt6 platforminputcontexts install directory (default: from qmake)")

if(Y2SKK_QT6_PLUGIN_DIR)
    set(_install_plugin_dir "${Y2SKK_QT6_PLUGIN_DIR}")
else()
    set(_install_plugin_dir "${_qt6_plugin_dir}/platforminputcontexts")
endif()

install(TARGETS y2skk-qt6-plugin
    LIBRARY DESTINATION "${_install_plugin_dir}"
)
