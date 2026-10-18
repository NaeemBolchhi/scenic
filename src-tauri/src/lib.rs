mod commands;
mod error;
mod image;
mod temp;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_scenic::init())
        .setup(|app| {
            // Leftovers from a crash or a killed process.
            temp::sweep(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::download_wallpaper,
            commands::set_wallpaper,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Scenic");
}
