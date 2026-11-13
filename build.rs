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

fn build_user(manifest: &PathBuf, out: &PathBuf, name: &str, script: &PathBuf) -> PathBuf {
    let source = manifest.join(format!("user/{}.S", name));
    let object = out.join(format!("{}.o", name));
    let elf = out.join(format!("{}.elf", name));
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", script.display());

    let mut assembler = Command::new("as");
    assembler.arg("--64").arg("-o").arg(&object).arg(&source);
    run(&mut assembler);

    let mut linker = Command::new("ld");
    linker
        .arg("-m")
        .arg("elf_x86_64")
        .arg("-z")
        .arg("max-page-size=4096")
        .arg("-T")
        .arg(script)
        .arg("-o")
        .arg(&elf)
        .arg(&object);
    run(&mut linker);

    let size = fs::metadata(&elf).expect("user ELF metadata").len();
    if size == 0 || size > 64 * 1024 {
        panic!("user ELF {} size {} is outside 1..=65536", name, size);
    }
    elf
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let script = manifest.join("user/user.ld");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", script.display());

    let init = build_user(&manifest, &out, "init", &script);
    let echo = build_user(&manifest, &out, "echo", &script);
    let bin = manifest.join("build/user/bin");
    fs::create_dir_all(&bin).expect("create build/user/bin");
    fs::copy(init, bin.join("init.elf")).expect("copy init.elf");
    fs::copy(echo, bin.join("echo.elf")).expect("copy echo.elf");
}
