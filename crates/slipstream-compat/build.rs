fn main() {
    println!("cargo:rerun-if-changed=native/native_probe.cc");
    let libraw = pkg_config::Config::new()
        .probe("libraw")
        .expect("Issue #20 probe requires system LibRaw development files");
    let jpeg = pkg_config::Config::new()
        .probe("libjpeg")
        .expect("Issue #20 probe requires system libjpeg development files");
    let vips = pkg_config::Config::new()
        .probe("vips")
        .expect("Issue #22 probe requires system libvips development files");
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").file("native/native_probe.cc");
    for include in libraw
        .include_paths
        .into_iter()
        .chain(jpeg.include_paths)
        .chain(vips.include_paths)
    {
        build.include(include);
    }
    build.compile("slipstream_native_probe");
}
