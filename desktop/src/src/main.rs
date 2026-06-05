//! Syscity Desktop entry point

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    syscity_desktop_lib::run();
}
