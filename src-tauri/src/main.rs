#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = wuwa_ini_tool_lib::run() {
        eprintln!("Wuwa ini Tool failed to start: {error}");
        std::process::exit(1);
    }
}
