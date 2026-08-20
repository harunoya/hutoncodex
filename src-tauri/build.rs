fn main() {
    println!("cargo:rerun-if-env-changed=CODEX_REMOTE_DISCORD_APP_ID");
    tauri_build::build()
}
