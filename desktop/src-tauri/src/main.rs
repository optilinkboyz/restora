// Suppress the console window on Windows in release builds, matching
// standard Tauri app conventions.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    restora_desktop_lib::run();
}
