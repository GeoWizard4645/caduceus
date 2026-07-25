// Hide the console window on Windows release builds. Caduceus is a tray app; a
// terminal appearing behind it would look like a bug.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    caduceus_lib::run()
}
