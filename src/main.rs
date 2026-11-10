#![cfg_attr(not(test), windows_subsystem = "windows")]

mod actions;
mod columns;
mod core;
mod devtools;
mod gfx;
mod grouping;
mod model;
mod pages;
mod pointer;
mod sampler;
mod settings;
mod sysinfo;
mod theme;
mod window;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--bench") => devtools::bench(args.get(1).map_or("bench.txt", String::as_str)),
        Some("--shot") => devtools::shot(&args[1..]),
        _ => window::run(),
    }
}
