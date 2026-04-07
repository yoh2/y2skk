fn main() {
    // Locate GTK+3 via pkg-config and emit linker flags.
    let gtk3 = pkg_config::Config::new()
        .atleast_version("3.24")
        .probe("gtk+-3.0")
        .expect("gtk+-3.0 >= 3.24 not found; install GTK3 development files");

    // Compile the C shim that handles GObject subclassing and GTK3 IM module
    // entry points.
    let mut build = cc::Build::new();
    build
        .file("src/c/im_module.c")
        .include("include")
        // GLib's GTypeInfo struct requires casting function pointers to incompatible
        // types; suppress that diagnostic since it's unavoidable GObject boilerplate.
        .flag_if_supported("-Wno-cast-function-type");

    for path in &gtk3.include_paths {
        build.include(path);
    }

    build.compile("y2skk_im_shim");

    // Re-run if the C source or header changes.
    println!("cargo:rerun-if-changed=src/c/im_module.c");
    println!("cargo:rerun-if-changed=include/y2skk_im.h");
}
