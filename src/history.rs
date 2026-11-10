//! Launch history for frecency ranking: how often and how recently each app
//! was launched, in `~/.local/share/pixie/history.toml`.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// When the summed counts pass this, every count is scaled down so old
/// favourites fade and the file stays small.
const MAX_TOTAL: f64 = 1000.0;
const AGING: f64 = 0.9;

#[derive(Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    apps: BTreeMap<String, Usage>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Usage {
    count: f64,
    /// Unix seconds of the latest launch.
    last: i64,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn path() -> PathBuf {
    gtk::glib::user_data_dir()
        .join("pixie")
        .join("history.toml")
}

impl History {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Launch count weighted by recency (zoxide's buckets).
    pub fn score(&self, id: &str, now: i64) -> f64 {
        let Some(usage) = self.apps.get(id) else {
            return 0.0;
        };
        let age = now - usage.last;
        let weight = if age < 3_600 {
            4.0 // within the hour
        } else if age < 86_400 {
            2.0 // today
        } else if age < 604_800 {
            0.5 // this week
        } else {
            0.25
        };
        usage.count * weight
    }

    pub fn record(&mut self, id: &str, now: i64) {
        let usage = self.apps.entry(id.to_string()).or_insert(Usage {
            count: 0.0,
            last: now,
        });
        usage.count += 1.0;
        usage.last = now;
        if self.apps.values().map(|u| u.count).sum::<f64>() > MAX_TOTAL {
            self.apps.values_mut().for_each(|u| u.count *= AGING);
            self.apps.retain(|_, u| u.count >= 1.0);
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = path();
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}
