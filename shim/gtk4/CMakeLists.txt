cmake_minimum_required(VERSION 3.16)
project(y2skk-gtk4-im VERSION 0.1.0)

# ── GTK4 ─────────────────────────────────────────────────────────────────────

find_package(PkgConfig REQUIRED)
pkg_check_modules(GTK4 REQUIRED gtk4>=4.0)

# Determine the GTK4 IM module install directory from pkg-config so the path
# is independent of CMAKE_INSTALL_PREFIX. Unlike GTK3 there is no per-binary-
# version subdirectory — modules live directly under $LIBDIR/gtk-4.0/immodules.
pkg_get_variable(_gtk4_libdir gtk4 libdir)
set(GTK4_IM_MODULE_DIR
    "${_gtk4_libdir}/gtk-4.0/immodules"
    CACHE PATH "GTK4 IM modules directory (override if non-standard)")

message(STATUS "GTK4 IM module directory: ${GTK4_IM_MODULE_DIR}")

# ── Cargo build ───────────────────────────────────────────────────────────────

set(_cargo_root "${CMAKE_SOURCE_DIR}/../..")

if(CMAKE_BUILD_TYPE STREQUAL "Release")
    set(_cargo_args --release -p adapter-gtk4)
    set(_cargo_out  "${_cargo_root}/target/release/libadapter_gtk4.so")
else()
    set(_cargo_args -p adapter-gtk4)
    set(_cargo_out  "${_cargo_root}/target/debug/libadapter_gtk4.so")
endif()

add_custom_target(adapter-gtk4 ALL
    COMMAND cargo build ${_cargo_args}
    WORKING_DIRECTORY "${_cargo_root}"
    BYPRODUCTS "${_cargo_out}"
    COMMENT "Building adapter-gtk4 via Cargo"
)

# ── Install ───────────────────────────────────────────────────────────────────

# GIO requires module file names to start with "lib" — that is why the file
# is renamed to libim-y2skk.so rather than im-y2skk.so used by the GTK3 build.
install(
    FILES "${_cargo_out}"
    DESTINATION "${GTK4_IM_MODULE_DIR}"
    RENAME "libim-y2skk.so"
)

# Whether to refresh the GIO module cache after installing the file.
# Set to OFF when installing into a staging/prefix directory (e.g. during packaging).
option(GTK4_UPDATE_GIOMODULE_CACHE
    "Run gio-querymodules on the install directory after installing the IM module"
    ON)

if(GTK4_UPDATE_GIOMODULE_CACHE)
    install(CODE "
        message(STATUS \"Updating GIO module cache for ${GTK4_IM_MODULE_DIR}...\")
        execute_process(
            COMMAND gio-querymodules \"${GTK4_IM_MODULE_DIR}\"
            RESULT_VARIABLE _rc
        )
        if(NOT _rc EQUAL 0)
            message(WARNING \"gio-querymodules failed (exit \${_rc}); GTK4 may not pick up the module\")
        endif()
    ")
endif()
