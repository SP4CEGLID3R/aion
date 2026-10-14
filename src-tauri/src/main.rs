// No console window behind the overlay in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    aion2_radar_lib::run();
}
