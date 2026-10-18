// The plugin exposes no JS commands of its own. The app crate defines the
// commands and calls this plugin through its Rust API, so the list is empty.
// `android_path` tells tauri-build to link the Kotlin library into the
// generated Android project (src-tauri/gen/android).
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
