//! `pixie`: starts the popup (`pixie-ui`, next to this binary) without
//! `__GLX_VENDOR_LIBRARY_NAME` in its environment.
//!
//! GTK links GStreamer's GL library, which links glvnd's libGLX. When that
//! variable is set, libGLX's constructor loads the named vendor library before
//! `main` runs; for NVIDIA that costs ~17 ms of every open, for a popup that
//! never uses GL. pixie-ui puts the variable back for the apps it launches.
//! This binary uses only std, so it doesn't load GTK itself.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

const VAR: &str = "__GLX_VENDOR_LIBRARY_NAME";

fn main() -> ExitCode {
    let ui = match std::env::current_exe() {
        Ok(exe) => exe.with_file_name("pixie-ui"),
        Err(e) => {
            eprintln!("pixie: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut command = Command::new(&ui);
    command.args(std::env::args_os().skip(1));
    if let Some(value) = std::env::var_os(VAR) {
        command.env_remove(VAR).env("PIXIE_SAVED_GLX_VENDOR", value);
    }
    let error = command.exec();
    eprintln!("pixie: could not start {}: {error}", ui.display());
    ExitCode::FAILURE
}
