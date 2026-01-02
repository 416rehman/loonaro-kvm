// build.rs - generates FFI bindings from libvmi.h
use std::env;
use std::path::PathBuf;

fn main() {
    // dynamic linking for now
    println!("cargo:rustc-link-lib=vmi");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-search=/usr/local/lib");
    
    // get glib flags via pkg-config
    let glib = pkg_config::Config::new()
        .probe("glib-2.0")
        .expect("glib-2.0 not found");
    
    // generate bindings
    let mut builder = bindgen::Builder::default()
        .header("/usr/local/include/libvmi/libvmi.h")
        .clang_arg("-I/usr/local/include")
        // add gcc headers for stddef.h
        .clang_arg("-I/usr/lib/gcc/x86_64-linux-gnu/13/include") 
        .allowlist_function("vmi_.*")
        .allowlist_type("vmi_.*")
        .allowlist_type("status_t")
        .allowlist_type("addr_t")
        .allowlist_type("win_ver_t")
        .allowlist_type("os_t")
        .allowlist_type("access_context_t")
        .allowlist_var("VMI_.*")
        .derive_debug(true)
        .derive_default(true);
    
    // add glib include paths
    for path in &glib.include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }
    
    let bindings = builder
        .generate()
        .expect("Unable to generate bindings");
    
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
