fn main() {
    println!("cargo:rerun-if-changed=native/raw_preview.cc");
    println!("cargo:rerun-if-changed=native/raw_preview.h");
    println!("cargo:rerun-if-changed=native/vips_preview.cc");
    println!("cargo:rerun-if-changed=native/vips_preview.h");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .file("native/raw_preview.cc")
        .file("native/vips_preview.cc");
    for package in ["libraw", "libjpeg", "vips"] {
        let library = pkg_config::Config::new()
            .probe(package)
            .unwrap_or_else(|_| panic!("Issue #22 requires system {package} development files"));
        for include in library.include_paths {
            build.include(include);
        }
    }
    build.compile("slipstream_raw_preview");
}
