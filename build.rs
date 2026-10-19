//! Stamps the Cargo target triple into the binary so the version cache
//! distinguishes native and translated (Rosetta) installations.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TARGET");
    match std::env::var("TARGET") {
        Ok(target) => println!("cargo:rustc-env=DINYD_BUILD_TARGET={target}"),
        Err(_) => panic!("build.rs: Cargo did not provide TARGET"),
    }
}
