fn main() {
    // Locate GTK4 via pkg-config and emit linker flags.
    let gtk4 = pkg_config::Config::new()
        .atleast_version("4.0")
        .probe("gtk4")
        .expect("gtk4 not found; install GTK4 development files");

    // Probe gio-2.0 explicitly so that pkg-config also emits the include
    // paths (we don't actually rely on its `-l` flags reaching DT_NEEDED;
    // see the linker workaround below).
    pkg_config::Config::new()
        .atleast_version("2.50")
        .probe("gio-2.0")
        .expect("gio-2.0 not found; install GLib development files");

    // Force libgio / libgobject / libglib / libgtk-4 into the cdylib's
    // DT_NEEDED, working around an `--as-needed` link-order trap.
    //
    // Rust's link command for a cdylib + cc::Build static archive is:
    //
    //     <Rust .o>  -l gtk-4 ... -l gio-2.0 ...  -l static=<shim>
    //                ^^^^^^^^^^^^^^^^^^^^^^^^^^^  ^^^^^^^^^^^^^^^^^
    //                shared libs (from pkg-cfg)   C shim (im_module.o)
    //
    // With the modern default `--as-needed` (e.g. GNU ld 2.46 on Gentoo
    // aarch64 + hardened toolchains), the shared libs are evaluated
    // *before* the static archive contributes its undefined symbols.
    // At that point the only undefined symbols are Rust's wrappers like
    // `_y2skk_g_io_module_load`, which none of the shared libs resolve,
    // so the linker silently drops every `-l` shared lib. The resulting
    // .so still links (cdylib tolerates undefined symbols at link time),
    // but it has no DT_NEEDED for libgio / libgtk-4, and dlopen during
    // `gio-querymodules` fails with
    //     undefined symbol: g_io_extension_point_implement
    // — which leaves no `giomodule.cache` and renders the IM module
    // invisible to GTK4 apps. (`amd64` happens to keep the libs in
    // DT_NEEDED via a linker / compiler-spec quirk; aarch64 hardened
    // does not, hence the platform-specific breakage.)
    //
    // `cargo:rustc-link-arg=` flags are appended at the very end of the
    // link command, *after* the static archive. By re-emitting the
    // needed `-l` flags there bracketed in --no-as-needed, the linker
    // sees them at a point where im_module.o's references to
    // `g_io_extension_point_implement` etc. are already undefined, and
    // `--no-as-needed` forces them into DT_NEEDED unconditionally.
    println!("cargo:rustc-link-arg=-Wl,--push-state,--no-as-needed");
    for lib in ["gio-2.0", "gobject-2.0", "glib-2.0", "gtk-4"] {
        println!("cargo:rustc-link-arg=-l{lib}");
    }
    println!("cargo:rustc-link-arg=-Wl,--pop-state");

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
