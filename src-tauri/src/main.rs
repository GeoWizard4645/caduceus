// Hide the console window on Windows release builds. Caduceus is a tray app; a
// terminal appearing behind it would look like a bug.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if caduceus_lib::wants_permission_primer() {
        caduceus_lib::run_permission_primer();
        return;
    }
    caduceus_lib::run()
}
