// Hide the spawned console on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    oxyris_desktop_lib::run();
}
