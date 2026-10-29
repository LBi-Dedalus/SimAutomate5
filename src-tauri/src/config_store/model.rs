//! Template and configuration types.
//!
//! `Template` keeps its historical JSON shape (flat `autobuild` / `autobuild_no_etb` booleans)
//! through the private `TemplateWire` struct, so config files and the frontend are unaffected.
use std::sync::Arc;
use serde::{Serialize};
use crate::auto_reply::{AutoReplyConfig, AutoReplyRuleSet};
use crate::templates::Template;


/// The configuration the app runs with: the parsed file (defaults for missing keys), the compiled
/// rules and the revision it was observed at. This is the single source of truth, for the backend
/// and the frontend. The revision is a counter owned by the `ConfigStore`: it grows on every
/// successful save, so a consumer (the frontend, the `ConnectionManager`) can discard a
/// configuration that arrives after a newer one.
#[derive(Debug, Clone, Serialize)]
pub struct RevisionedConfig {
    pub revision: u64,
    pub templates: Vec<Template>,
    pub auto_reply: AutoReplyConfig,
    /// Compiled from `templates` and `auto_reply`; for the backend only.
    #[serde(skip)]
    pub rules: Arc<AutoReplyRuleSet>,
}

/// `rules` is derived from the other fields, so it is not compared.
impl PartialEq for RevisionedConfig {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.templates == other.templates
            && self.auto_reply == other.auto_reply
    }
}
