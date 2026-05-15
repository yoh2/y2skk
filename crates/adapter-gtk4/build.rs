fn main() {
    // Locate GTK4 via pkg-config and emit linker flags.
    let gtk4 = pkg_config::Config::new()
        .atleast_version("4.0")
        .probe("gtk4")
        .expect("gtk4 not found; install GTK4 development files");

    // Compile the C shim that handles GObject subclassing and the GIO module
    // entry points used by GTK4's `gtk-im-module` extension point.
    let mut build = cc::Build::new();
    build
        .file("src/c/im_module.c")
        .include("include")
        // GLib's GTypeInfo struct requires casting function pointers to incompatible
        // types; suppress that diagnostic since it's unavoidable GObject boilerplate.
        .flag_if_supported("-Wno-cast-function-type");

    for path in &gtk4.include_paths {
        build.include(path);
    }

    build.compile("y2skk_im_shim_gtk4");

    // Re-run if the C source or header changes.
    println!("cargo:rerun-if-changed=src/c/im_module.c");
    println!("cargo:rerun-if-changed=include/y2skk_im.h");
}
