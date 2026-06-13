// 發行版隱藏主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    desktop_pet_ai_lib::run()
}
