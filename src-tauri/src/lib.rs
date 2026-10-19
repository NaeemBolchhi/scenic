mod cache;
mod commands;
mod error;
mod fetch;
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
            commands::set_system_bars,
            commands::store_image_cache,
            commands::erase_image_cache,
            commands::clear_expired_image_cache,
            commands::load_image_cache,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Scenic");
}
