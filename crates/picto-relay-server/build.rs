//! Rebuilds the server when the web client it embeds changes.

fn main() {
    // `services/web/dist` is produced by `just web-build`. Until it exists Cargo treats
    // it as always changed, so the server is recompiled on every build.
    println!("cargo:rerun-if-changed=../../services/web/dist");
}
