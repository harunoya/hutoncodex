fn main() {
    println!("cargo:rerun-if-env-changed=HUTONCODEX_DISCORD_APP_ID");
    tauri_build::build()
}
