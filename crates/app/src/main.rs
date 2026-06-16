// On Windows, release builds should not spawn a console window. No effect on
// Linux (our v1 target), but kept so the binary behaves correctly everywhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    app_lib::run();
}
