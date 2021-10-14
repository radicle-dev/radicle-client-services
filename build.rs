use std::process::Command;

fn main() {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--short")
        .arg("HEAD")
        .output()
        .expect("git command failed; is git installed?");

    if !output.status.success() {
        panic!("git command returned non-zero exit code: {:?}", &output);
    }

    let hash = match String::from_utf8(output.stdout) {
        Ok(hash) => hash,
        Err(e) => panic!("UTF-8 decoding failed: {:?}", e),
    };

    println!("cargo:rustc-env=GIT_HEAD={}", hash);
}
