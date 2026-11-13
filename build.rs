use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to run {:?}: {}", command, error));
    if !status.success() {
        panic!("command failed: {:?}", command);
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let source = manifest.join("user/hello.S");
    let script = manifest.join("user/user.ld");
    let object = out.join("hello.o");
    let elf = out.join("hello.elf");
    let binary = out.join("hello.bin");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut assembler = Command::new("as");
    assembler.arg("--64").arg("-o").arg(&object).arg(&source);
    run(&mut assembler);

    let mut linker = Command::new("ld");
    linker
        .arg("-m")
        .arg("elf_x86_64")
        .arg("-T")
        .arg(&script)
        .arg("-o")
        .arg(&elf)
        .arg(&object);
    run(&mut linker);

    let mut objcopy = Command::new("objcopy");
    objcopy.arg("-O").arg("binary").arg(&elf).arg(&binary);
    run(&mut objcopy);

    let size = fs::metadata(&binary).expect("user blob metadata").len();
    if size == 0 || size > 64 * 1024 {
        panic!("user blob size {} is outside 1..=65536", size);
    }
}
