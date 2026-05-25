//! Skills system configuration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Enable the skills system.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Automatically save complex interactions as skills.
    #[serde(default = "default_true")]
    pub auto_save_interactions: bool,
    /// Automatically suggest skill improvements after N uses.
    #[serde(default = "default_true")]
    pub auto_suggest_improvements: bool,
    /// Days between improvement suggestion checks.
    #[serde(default = "default_improvement_interval_days")]
    pub improvement_interval_days: u32,
}

fn default_true() -> bool {
    true
}

fn default_improvement_interval_days() -> u32 {
    7
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_save_interactions: true,
            auto_suggest_improvements: true,
            improvement_interval_days: 7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_config_defaults() {
        let c = SkillsConfig::default();
        assert!(c.enabled);
        assert!(c.auto_save_interactions);
        assert!(c.auto_suggest_improvements);
        assert_eq!(c.improvement_interval_days, 7);
    }
}
