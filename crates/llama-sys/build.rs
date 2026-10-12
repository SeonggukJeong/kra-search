use std::{env, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("vendor/llama.cpp");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=vendor/llama.cpp/include/llama.h");

    let mut cfg = cmake::Config::new(&src);
    for (k, v) in [
        ("LLAMA_CURL", "OFF"),
        ("LLAMA_BUILD_TESTS", "OFF"),
        ("LLAMA_BUILD_EXAMPLES", "OFF"),
        ("LLAMA_BUILD_TOOLS", "OFF"),
        ("LLAMA_BUILD_SERVER", "OFF"),
        ("LLAMA_BUILD_APP", "OFF"),
        ("LLAMA_BUILD_COMMON", "OFF"),
        ("GGML_OPENMP", "OFF"),
        ("GGML_NATIVE", "OFF"),
        ("GGML_METAL", "OFF"),
        ("GGML_BLAS", "OFF"),
    ] {
        cfg.define(k, v);
    }
    cfg.profile("Release");
    if os == "windows" {
        cfg.define("BUILD_SHARED_LIBS", "ON")
            .define("GGML_BACKEND_DL", "ON")
            .define("GGML_CPU_ALL_VARIANTS", "ON")
            .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded");
    } else {
        cfg.define("BUILD_SHARED_LIBS", "OFF");
    }
    let dst = cfg.build();
    for d in ["lib", "lib64"] {
        println!("cargo:rustc-link-search=native={}", dst.join(d).display());
    }

    if os == "windows" {
        for l in ["llama", "ggml", "ggml-base"] {
            println!("cargo:rustc-link-lib=dylib={l}");
        }
        println!("cargo:dll_dir={}", dst.join("bin").display());
    } else {
        for l in ["llama", "ggml", "ggml-cpu", "ggml-base"] {
            println!("cargo:rustc-link-lib=static={l}");
        }
        if os == "macos" {
            println!("cargo:rustc-link-lib=c++");
            println!("cargo:rustc-link-lib=framework=Accelerate");
        } else {
            println!("cargo:rustc-link-lib=stdc++");
        }
    }

    bindgen::Builder::default()
        .header(manifest.join("wrapper.h").to_string_lossy())
        .clang_arg(format!("-I{}", src.join("include").display()))
        .clang_arg(format!("-I{}", src.join("ggml/include").display()))
        .allowlist_function("llama_.*")
        .allowlist_function("ggml_backend_load_all_from_path")
        .allowlist_type("llama_.*")
        .allowlist_var("LLAMA_.*")
        .generate()
        .expect("bindgen failed")
        .write_to_file(PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs"))
        .unwrap();
}
