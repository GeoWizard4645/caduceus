// Hide the console window on Windows release builds. Orbit is a tray app; a
// terminal appearing behind it would look like a bug.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    orbit_lib::run()
}
