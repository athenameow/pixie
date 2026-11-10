//! Starting things. Every process is spawned with GLib's double fork in a new
//! session, so it is re-parented away from pixie at once, never waits on it,
//! and isn't touched by signals sent to pixie's process group.
//!
//! Under UWSM, apps go through `uwsm app`, which puts each in its own systemd
//! scope and handles `Terminal=` (via `xdg-terminals.list`) and `Path=`.
//! Otherwise GIO launches them, except `Terminal=true` apps, which open in
//! `$TERMINAL` (default kitty) because GIO's own terminal list lacks kitty.

use anyhow::{Context, Result, anyhow};
use gio_unix::DesktopAppInfo;
use gtk::gio::{self, prelude::*};
use gtk::glib;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Environment variables pixie set for itself at startup; launched programs
/// get the environment pixie was started with.
pub static OWN_VARS: OnceLock<Vec<&'static str>> = OnceLock::new();

pub enum Launch<'a> {
    App {
        id: &'a str,
        action: Option<&'a str>,
    },
    Shell {
        command: &'a str,
        terminal: bool,
    },
    Copy(&'a str),
}

pub fn run(what: Launch, context: &gio::AppLaunchContext) -> Result<()> {
    let uwsm = uwsm();
    match what {
        Launch::App { id, action } if uwsm => {
            let target = match action {
                Some(action) => format!("{id}:{action}"),
                None => id.to_string(),
            };
            spawn(
                &["uwsm".into(), "app".into(), "--".into(), target.into()],
                None,
            )
        }
        Launch::App { id, action } => launch_gio(id, action, context),
        Launch::Shell { command, terminal } => {
            let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
            let mut argv: Vec<OsString> = Vec::new();
            if uwsm {
                argv.extend(["uwsm".into(), "app".into(), "--".into()]);
            }
            if terminal {
                argv.extend([terminal_program(), "--hold".into()]);
            }
            argv.extend([shell, "-c".into(), command.into()]);
            spawn(&argv, Some(&glib::home_dir()))
        }
        Launch::Copy(text) => spawn(&["wl-copy".into(), "--".into(), text.into()], None),
    }
}

/// Whether the session runs under UWSM and its launcher is available.
fn uwsm() -> bool {
    std::env::var_os("PIXIE_NO_UWSM").is_none()
        && ["UWSM_FINALIZE_VARNAMES", "UWSM_WAIT_VARNAMES"]
            .iter()
            .any(|var| std::env::var_os(var).is_some())
        && glib::find_program_in_path("uwsm").is_some()
}

fn terminal_program() -> OsString {
    std::env::var_os("TERMINAL").unwrap_or_else(|| "kitty".into())
}

fn launch_gio(id: &str, action: Option<&str>, context: &gio::AppLaunchContext) -> Result<()> {
    let info = DesktopAppInfo::new(id).ok_or_else(|| anyhow!("{id} not found"))?;
    for var in OWN_VARS.get().into_iter().flatten() {
        context.unsetenv(var);
    }
    if let Some(action) = action {
        info.launch_action(action, Some(context));
        return Ok(());
    }
    if info.boolean("Terminal") {
        let commandline = info
            .commandline()
            .ok_or_else(|| anyhow!("{id} has no Exec line"))?;
        let mut argv = vec![terminal_program(), "--".into()];
        argv.extend(exec_argv(&commandline.to_string_lossy())?);
        let cwd = info.string("Path").map(|p| PathBuf::from(p.as_str()));
        return spawn(&argv, cwd.as_deref());
    }
    info.launch(&[], Some(context)).context("launch failed")
}

/// Splits an `Exec` line into arguments, dropping field codes (`%f`, `%U`, …),
/// since nothing is opened with the app.
fn exec_argv(exec: &str) -> Result<Vec<OsString>> {
    let args = glib::shell_parse_argv(exec).context("invalid Exec line")?;
    Ok(args
        .into_iter()
        .filter(|arg| {
            let arg = arg.to_string_lossy();
            !(arg.len() == 2 && arg.starts_with('%') && arg != "%%")
        })
        .map(|arg| arg.to_string_lossy().replace("%%", "%").into())
        .collect())
}

/// Runs `argv` fully detached: GLib forks twice, so the program is
/// re-parented to the session's subreaper (systemd) and pixie can exit.
fn spawn(argv: &[OsString], cwd: Option<&Path>) -> Result<()> {
    let own = OWN_VARS.get().map(Vec::as_slice).unwrap_or_default();
    let env: Vec<PathBuf> = std::env::vars_os()
        .filter(|(key, _)| !own.iter().any(|own| key == own))
        .map(|(key, value)| {
            let mut pair = key;
            pair.push("=");
            pair.push(value);
            PathBuf::from(pair)
        })
        .collect();
    let env: Vec<&Path> = env.iter().map(PathBuf::as_path).collect();
    let argv: Vec<&Path> = argv.iter().map(Path::new).collect();
    crate::mark(&format!("spawn {argv:?}"));
    glib::spawn_async(
        cwd,
        &argv,
        &env,
        glib::SpawnFlags::SEARCH_PATH,
        Some(Box::new(|| {
            // SAFETY: runs in the forked child before exec; setsid is async-signal-safe.
            unsafe { libc::setsid() };
        })),
    )
    .with_context(|| format!("could not start {}", argv[0].display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn exec_field_codes() {
        let argv = super::exec_argv("btop --utf-force %U '100%%' \"a b\"").unwrap();
        assert_eq!(argv, ["btop", "--utf-force", "100%", "a b"]);
    }
}
