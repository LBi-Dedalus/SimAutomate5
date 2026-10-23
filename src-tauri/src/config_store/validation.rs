//! Validation of templates and of their cross-references with the auto reply rules.

use std::path::Path;
use std::sync::Arc;

use super::model::{Template, TemplateId};
use crate::auto_reply::{AutoReplyAction, AutoReplyConfig, AutoReplyRuleSet};

fn is_valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn validate_templates(templates: &[Template]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for (index, template) in templates.iter().enumerate() {
        let label = format!("template #{}", index + 1);
        if template.id.as_str().trim().is_empty() {
            return Err(format!("{label}: id must not be empty"));
        }
        if !ids.insert(template.id.as_str()) {
            return Err(format!("{label}: duplicate id \"{}\"", template.id));
        }
        if template.name.trim().is_empty() {
            return Err(format!("{label}: name must not be empty"));
        }
        let mut names = std::collections::HashSet::new();
        for variable in &template.variables {
            if !is_valid_variable_name(&variable.name) {
                return Err(format!(
                    "{label}: invalid variable name \"{}\"",
                    variable.name
                ));
            }
            if !names.insert(variable.name.as_str()) {
                return Err(format!("{label}: duplicate variable \"{}\"", variable.name));
            }
        }
    }
    Ok(())
}

/// Context-free errors.
pub(super) fn compile_with(
    config: &AutoReplyConfig,
    templates: &[Template],
) -> Result<Arc<AutoReplyRuleSet>, String> {
    AutoReplyRuleSet::compile(config, templates)
        .map(Arc::new)
        .map_err(|err| err.to_string())
}

/// `compile_with` for the config at `path`: the error names the file.
pub(super) fn compile_at(
    path: &Path,
    config: &AutoReplyConfig,
    templates: &[Template],
) -> Result<Arc<AutoReplyRuleSet>, String> {
    compile_with(config, templates).map_err(|err| {
        format!(
            "Invalid auto reply configuration in {}: {err}",
            path.display()
        )
    })
}

/// First rule (enabled or not) whose template is absent from `templates`, as (template id, rule name).
pub(super) fn first_rule_missing_template(
    config: &AutoReplyConfig,
    templates: &[Template],
) -> Option<(TemplateId, String)> {
    config.rules().iter().find_map(|rule| match rule.action() {
        AutoReplyAction::Template { template_id }
            if !templates.iter().any(|t| &t.id == template_id) =>
        {
            Some((template_id.clone(), rule.name().to_string()))
        }
        _ => None,
    })
}
