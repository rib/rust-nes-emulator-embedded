

use clap::Parser;

use nes_emulator_shell as nes_shell;

fn main() {
    // Make sure panics are logged using `console.error`.
    console_error_panic_hook::set_once();

    // Redirect tracing to console.log and friends:
    tracing_wasm::set_as_global_default();
    wasm_logger::init(wasm_logger::Config::default());
    log::debug!("Test 1");

    let args = nes_shell::Args::parse();

    nes_shell::ui::eframe::web_ui_main(args, "nes_emulator_canvas");
}