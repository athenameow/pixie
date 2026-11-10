//! Orders apps for a query: fuzzy match quality (nucleo) blended with frecency.

use crate::apps::App;
use crate::history::History;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

/// How much a match in each field counts: name, generic name, keywords,
/// executable, all fields joined.
const FIELD_WEIGHTS: [f64; 5] = [1.0, 0.8, 0.7, 0.7, 0.5];
/// Extra score when the name starts with the query, so typing "fi" puts
/// Firefox above "Profile manager".
const PREFIX_BONUS: f64 = 24.0;
/// Frecency adds at most this much, about two well-placed matched characters:
/// enough to order similar matches by habit, not enough to beat a clearly
/// better match.
const FRECENCY_SCALE: f64 = 10.0;
const FRECENCY_CAP: f64 = 36.0;

pub struct Ranker {
    matcher: Matcher,
}

impl Ranker {
    pub fn new() -> Self {
        let mut config = Config::DEFAULT;
        config.prefer_prefix = true;
        Self {
            matcher: Matcher::new(config),
        }
    }

    /// Indices into `apps`, best first. An empty query lists every app by
    /// frecency, then alphabetically.
    pub fn rank(&mut self, apps: &[App], history: &History, query: &str, now: i64) -> Vec<usize> {
        let query = query.trim();
        let frecency = |app: &App| history.score(&app.id, now);
        let mut hits: Vec<(usize, f64)> = if query.is_empty() {
            apps.iter()
                .enumerate()
                .map(|(i, app)| (i, frecency(app)))
                .collect()
        } else {
            let pattern = Pattern::new(
                query,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
            );
            let multi_word = pattern.atoms.len() > 1;
            let query_lower = query.to_lowercase();
            apps.iter()
                .enumerate()
                .filter_map(|(i, app)| {
                    let fields = if multi_word {
                        &app.fields[..]
                    } else {
                        &app.fields[..4]
                    };
                    let score = fields
                        .iter()
                        .zip(FIELD_WEIGHTS)
                        .filter_map(|(field, weight)| {
                            pattern
                                .score(field.slice(..), &mut self.matcher)
                                .map(|s| s as f64 * weight)
                        })
                        .reduce(f64::max)?;
                    let prefix = if app.name_lower.starts_with(&query_lower) {
                        PREFIX_BONUS
                    } else {
                        0.0
                    };
                    let habit = (FRECENCY_SCALE * frecency(app).ln_1p()).min(FRECENCY_CAP);
                    Some((i, score + prefix + habit))
                })
                .collect()
        };
        // Ties: shorter names first when searching (closer to the query), then A–Z.
        hits.sort_by(|&(a, score_a), &(b, score_b)| {
            let (a, b) = (&apps[a], &apps[b]);
            let shorter = if query.is_empty() {
                std::cmp::Ordering::Equal
            } else {
                a.name.len().cmp(&b.name.len())
            };
            score_b
                .total_cmp(&score_a)
                .then(shorter)
                .then_with(|| a.name_lower.cmp(&b.name_lower))
        });
        hits.into_iter().map(|(i, _)| i).collect()
    }
}
