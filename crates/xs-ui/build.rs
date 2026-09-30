use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=resources.gresource.xml");
    println!("cargo:rerun-if-changed=../../packaging/io.github.tymonoman.Extraspace.svg");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"))
        .join("extraspace.gresource");
    let status = Command::new("glib-compile-resources")
        .args([
            "resources.gresource.xml",
            "--sourcedir=../../packaging",
            "--target",
        ])
        .arg(output)
        .status()
        .expect("glib-compile-resources is required; run scripts/setup.sh");
    assert!(
        status.success(),
        "could not compile the Extraspace icon resource"
    );
}
