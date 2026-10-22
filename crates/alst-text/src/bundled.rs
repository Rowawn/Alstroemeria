//! First-party language extensions shipped inside the app. Temporary: the M3 registry
//! (`alst-ext`) discovers and loads these from their folders instead.

use crate::{LanguageDef, Result};

const MANIFESTS: &[&str] = &[include_str!("../../../extensions/lang-es/extension.json")];

pub fn languages() -> Result<Vec<LanguageDef>> {
    MANIFESTS.iter().map(|m| LanguageDef::from_manifest(m)).collect()
}
