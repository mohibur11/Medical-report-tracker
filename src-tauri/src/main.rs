// Windows: suppress the console window in release builds. In debug it stays,
// because that is where Rust panics and eprintln! land.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    mrt_lib::run()
}
