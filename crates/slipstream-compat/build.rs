fn main() {
    println!("cargo:rerun-if-changed=native/native_probe.cc");
    pkg_config::Config::new()
        .probe("libraw")
        .expect("Issue #20 probe requires system LibRaw development files");
    pkg_config::Config::new()
        .probe("libjpeg")
        .expect("Issue #20 probe requires system libjpeg development files");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("native/native_probe.cc")
        .compile("slipstream_native_probe");
}
