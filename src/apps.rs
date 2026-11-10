//! The app index, read through GIO so `NoDisplay`, `Hidden`,
//! `OnlyShowIn`/`NotShowIn`, overrides and localised names are handled for us.
//!
//! Built on a worker thread while GTK starts, so it only holds plain data;
//! `DesktopAppInfo` objects are looked up again by ID when needed.

use gio_unix::DesktopAppInfo;
use gtk::gio::{self, prelude::*};
use nucleo_matcher::Utf32String;

/// Desktop entry of pixie itself (if installed), never listed.
const SELF_ID: &str = "io.athena.pixie.desktop";

/// Launchers whose name says nothing about the app; the desktop ID is
/// indexed instead (e.g. `flatpak run com.discordapp.Discord`).
const WRAPPERS: &[&str] = &[
    "flatpak",
    "env",
    "sh",
    "bash",
    "gtk-launch",
    "uwsm",
    "app2unit",
    "snap",
];

pub struct App {
    /// Desktop ID, e.g. `brave-browser.desktop`.
    pub id: String,
    pub name: String,
    /// Generic name, or the comment if there is none.
    pub subtitle: String,
    /// `gio::Icon` serialised with `to_string()`, rebuilt when a row shows it.
    pub icon: Option<String>,
    pub has_actions: bool,
    pub name_lower: String,
    /// Search fields: name, generic name, keywords, executable, and all of them
    /// joined (for multi-word queries that span fields).
    pub fields: [Utf32String; 5],
}

pub fn load() -> Vec<App> {
    gio::AppInfo::all()
        .into_iter()
        .filter(|info| info.should_show())
        .filter_map(|info| {
            let info = info.downcast::<DesktopAppInfo>().ok()?;
            let id = info.id()?.to_string();
            if id == SELF_ID {
                return None;
            }
            let name = info.name().to_string();
            let generic = info
                .generic_name()
                .map(|g| g.to_string())
                .filter(|g| !g.eq_ignore_ascii_case(&name))
                .unwrap_or_default();
            let comment = info
                .description()
                .map(|c| c.to_string())
                .unwrap_or_default();
            let subtitle = if generic.is_empty() {
                comment
            } else {
                generic.clone()
            };
            let keywords = info.keywords().join(" ");
            let exe = info
                .executable()
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let exe = if WRAPPERS.contains(&exe.as_str()) {
                id.trim_end_matches(".desktop")
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            } else {
                exe
            };
            let all = format!("{name} {generic} {keywords} {exe}");
            Some(App {
                icon: info
                    .icon()
                    .and_then(|icon| icon.to_string())
                    .map(|s| s.to_string()),
                has_actions: !info.list_actions().is_empty(),
                name_lower: name.to_lowercase(),
                fields: [
                    Utf32String::from(name.as_str()),
                    Utf32String::from(generic.as_str()),
                    Utf32String::from(keywords.as_str()),
                    Utf32String::from(exe.as_str()),
                    Utf32String::from(all.as_str()),
                ],
                id,
                name,
                subtitle,
            })
        })
        .collect()
}

/// A desktop action, e.g. Brave's "New private window".
pub struct Action {
    pub id: String,
    pub name: String,
}

pub fn actions(id: &str) -> Vec<Action> {
    let Some(info) = DesktopAppInfo::new(id) else {
        return Vec::new();
    };
    info.list_actions()
        .into_iter()
        .map(|action| Action {
            name: info.action_name(&action).to_string(),
            id: action.to_string(),
        })
        .collect()
}

/// The icon for a row, or a generic executable icon when the theme has
/// nothing under the app's name. (Appending the fallback as a second name
/// doesn't work: GTK tries every name in a theme before the themes it
/// inherits, so Papirus-Dark's own fallback would beat an app icon that lives
/// in Papirus.)
pub fn icon(serialized: Option<&str>, theme: &gtk::IconTheme) -> gio::Icon {
    serialized
        .and_then(|s| gio::Icon::for_string(s).ok())
        .filter(|icon| theme.has_gicon(icon))
        .unwrap_or_else(|| gio::ThemedIcon::new("application-x-executable").upcast())
}
