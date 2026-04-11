cmake_minimum_required(VERSION 3.16)
project(y2skk-gtk3-im VERSION 0.1.0)

# ── GTK3 ─────────────────────────────────────────────────────────────────────

find_package(PkgConfig REQUIRED)
pkg_check_modules(GTK3 REQUIRED gtk+-3.0>=3.24)

# Determine the IM module install directory from pkg-config so the path is
# independent of CMAKE_INSTALL_PREFIX.
pkg_get_variable(_gtk3_libdir       gtk+-3.0 libdir)
pkg_get_variable(_gtk3_binary_ver   gtk+-3.0 gtk_binary_version)
set(GTK3_IM_MODULE_DIR
    "${_gtk3_libdir}/gtk-3.0/${_gtk3_binary_ver}/immodules"
    CACHE PATH "GTK3 IM modules directory (override if non-standard)")

message(STATUS "GTK3 IM module directory: ${GTK3_IM_MODULE_DIR}")

# ── Cargo build ───────────────────────────────────────────────────────────────

set(_cargo_root "${CMAKE_SOURCE_DIR}/../..")

if(CMAKE_BUILD_TYPE STREQUAL "Release")
    set(_cargo_args --release -p adapter-gtk3)
    set(_cargo_out  "${_cargo_root}/target/release/libadapter_gtk3.so")
else()
    set(_cargo_args -p adapter-gtk3)
    set(_cargo_out  "${_cargo_root}/target/debug/libadapter_gtk3.so")
endif()

add_custom_target(adapter-gtk3 ALL
    COMMAND cargo build ${_cargo_args}
    WORKING_DIRECTORY "${_cargo_root}"
    BYPRODUCTS "${_cargo_out}"
    COMMENT "Building adapter-gtk3 via Cargo"
)

# ── Install ───────────────────────────────────────────────────────────────────

install(
    FILES "${_cargo_out}"
    DESTINATION "${GTK3_IM_MODULE_DIR}"
    RENAME "im-y2skk.so"
)

# Refresh the GTK3 IM module cache after installing the file.
install(CODE "
    message(STATUS \"Updating GTK3 IM module cache...\")
    execute_process(
        COMMAND gtk-query-immodules-3.0 --update-cache
        RESULT_VARIABLE _rc
    )
    if(NOT _rc EQUAL 0)
        message(WARNING \"gtk-query-immodules-3.0 --update-cache failed (exit \${_rc})\")
    endif()
")
