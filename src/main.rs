#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod devtools;
mod i18n;
mod pages;
mod settings;
mod system;
mod ui;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--bench") => devtools::bench(args.get(1).map_or("bench.txt", String::as_str)),
        Some("--shot") => devtools::shot(&args[1..]),
        _ => app::run(),
    }
}
