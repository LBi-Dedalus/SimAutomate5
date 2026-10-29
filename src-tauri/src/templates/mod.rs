use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Identifier of a saved template. Serialized as the bare string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TemplateId(String);

impl TemplateId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TemplateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for TemplateId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for TemplateId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl From<&str> for TemplateId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

impl PartialEq<str> for TemplateId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for TemplateId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateVariable {
    pub name: String,
    #[serde(default)]
    pub default: String,
}

/// Whether the UI runs the resolved message through Autobuild (MLLP/ASTM framing) before
/// sending it or loading it in the composer. Auto-reply rules ignore it.
///
/// The wire format is two booleans (`autobuild`, `autobuild_no_etb`). The frontend keeps the
/// "No ETB" choice while Autobuild is switched off, so `Disabled` remembers it: this keeps the
/// (`autobuild: false`, `autobuild_no_etb: true`) state lossless on a round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutobuildMode {
    Disabled { retained_no_etb: bool },
    Enabled { no_etb: bool },
}

impl AutobuildMode {
    /// Disabled, with no remembered "No ETB" choice.
    pub const DISABLED: Self = Self::Disabled {
        retained_no_etb: false,
    };

    /// The `(autobuild, autobuild_no_etb)` wire booleans.
    pub fn to_flags(self) -> (bool, bool) {
        match self {
            Self::Disabled { retained_no_etb } => (false, retained_no_etb),
            Self::Enabled { no_etb } => (true, no_etb),
        }
    }

    pub fn from_flags(autobuild: bool, no_etb: bool) -> Self {
        if autobuild {
            Self::Enabled { no_etb }
        } else {
            Self::Disabled {
                retained_no_etb: no_etb,
            }
        }
    }
}

impl Default for AutobuildMode {
    fn default() -> Self {
        Self::DISABLED
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "TemplateWire", into = "TemplateWire")]
pub struct Template {
    pub id: TemplateId,
    pub name: String,
    pub description: String,
    pub payload: String,
    pub variables: Vec<TemplateVariable>,
    pub autobuild: AutobuildMode,
}

/// The JSON shape of a `Template`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateWire {
    id: TemplateId,
    name: String,
    #[serde(default)]
    description: String,
    payload: String,
    #[serde(default)]
    variables: Vec<TemplateVariable>,
    #[serde(default)]
    autobuild: bool,
    /// Only meaningful when `autobuild` is true.
    #[serde(default)]
    autobuild_no_etb: bool,
}

impl From<TemplateWire> for Template {
    fn from(wire: TemplateWire) -> Self {
        Self {
            id: wire.id,
            name: wire.name,
            description: wire.description,
            payload: wire.payload,
            variables: wire.variables,
            autobuild: AutobuildMode::from_flags(wire.autobuild, wire.autobuild_no_etb),
        }
    }
}

impl From<Template> for TemplateWire {
    fn from(template: Template) -> Self {
        let (autobuild, autobuild_no_etb) = template.autobuild.to_flags();
        Self {
            id: template.id,
            name: template.name,
            description: template.description,
            payload: template.payload,
            variables: template.variables,
            autobuild,
            autobuild_no_etb,
        }
    }
}
