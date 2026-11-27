fn main() {
    let mut build = cc::Build::new();
    build
        .file("src/gui_dispatch.c")
        .include("src")
        .define("SUPER_PROVIDER_API", Some("static"))
        .std("c11");
    if cfg!(unix) {
        build.define("_DEFAULT_SOURCE", None);
    }
    build.warnings(false).compile("super_gui_provider");
    println!("cargo:rerun-if-changed=src/gui_dispatch.c");
    println!("cargo:rerun-if-changed=src/gtk_provider.c");
    println!("cargo:rerun-if-changed=src/win32_provider.c");
    println!("cargo:rerun-if-changed=src/cocoa_provider.c");
    println!("cargo:rerun-if-changed=src/gui_doc.h");
    println!("cargo:rerun-if-changed=src/gui_provider.h");
    if cfg!(windows) {
        println!("cargo:rustc-link-lib=comctl32");
        println!("cargo:rustc-link-lib=gdi32");
        println!("cargo:rustc-link-lib=user32");
    }
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
    }
}
