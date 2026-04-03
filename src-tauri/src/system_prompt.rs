//! Section-based system prompt builder for NexiBot.
//!
//! Sections are ordered, labeled as cacheable or dynamic, and assembled
//! into a single string for the API. A [DYNAMIC_BOUNDARY] marker separates
//! the cacheable prefix from dynamic sections, enabling Anthropic prompt caching.

/// A single section of the system prompt.
#[derive(Debug, Clone)]
pub struct PromptSection {
    pub key: String,
    pub content: String,
    /// Can be cached at the API level (static between turns).
    pub cacheable: bool,
    /// Higher priority sections are retained during compaction.
    pub priority: u8,
}

/// Channel the prompt is being built for (affects formatting rules).
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelContext {
    Gui,
    Telegram,
    Voice,
    Discord,
    Headless,
    Other,
}

pub struct SystemPromptBuilder {
    sections: Vec<PromptSection>,
    channel: ChannelContext,
}

impl SystemPromptBuilder {
    pub fn new(channel: ChannelContext) -> Self {
        SystemPromptBuilder { sections: Vec::new(), channel }
    }

    /// Add a section to the prompt.
    pub fn add_section(&mut self, key: &str, content: impl Into<String>, cacheable: bool, priority: u8) {
        self.sections.push(PromptSection {
            key: key.to_string(),
            content: content.into(),
            cacheable,
            priority,
        });
    }

    /// Build the full prompt string.
    /// Cacheable sections appear before [DYNAMIC_BOUNDARY].
    pub fn build(&self) -> String {
        let mut parts = Vec::new();
        let mut boundary_inserted = false;

        for section in &self.sections {
            if !boundary_inserted && !section.cacheable {
                parts.push("[DYNAMIC_BOUNDARY]".to_string());
                boundary_inserted = true;
            }
            if !section.content.trim().is_empty() {
                parts.push(section.content.clone());
            }
        }
        parts.join("\n\n")
    }

    /// Channel-specific formatting rules injected as a section.
    pub fn channel_rules_section(&self) -> String {
        match &self.channel {
            ChannelContext::Telegram => {
                "## Channel Rules (Telegram)\n\
                 - Use HTML formatting only: <b>, <i>, <code>, <pre>, <a>\n\
                 - Never use markdown (no **, no __, no backtick blocks with ```)\n\
                 - Keep responses concise; Telegram has message length limits\n\
                 - Use <pre><code class=\"language-rust\">...</code></pre> for code blocks".to_string()
            }
            ChannelContext::Voice => {
                "## Channel Rules (Voice)\n\
                 - Respond in natural spoken language only\n\
                 - Do NOT read file paths, function signatures, or code verbatim\n\
                 - Summarize technical content conversationally\n\
                 - No markdown, no slashes, no special characters\n\
                 - Keep responses short (1-3 sentences unless asked for detail)".to_string()
            }
            ChannelContext::Discord => {
                "## Channel Rules (Discord)\n\
                 - Use Discord markdown formatting\n\
                 - Code blocks with triple backticks and language identifiers\n\
                 - Keep responses under 2000 characters per message".to_string()
            }
            _ => String::new(), // Gui, Headless, Other: no special rules
        }
    }

    /// Build with standard sections for a coding agent session.
    pub fn build_coding_session(
        channel: ChannelContext,
        soul_prompt: Option<&str>,
        git_context: Option<&str>,
        memory: Option<&str>,
        mcp_instructions: Option<&str>,
        plan_mode_active: bool,
        tool_descriptions: &str,
    ) -> String {
        let mut builder = SystemPromptBuilder::new(channel);

        builder.add_section("identity", IDENTITY_PROMPT, true, 100);
        builder.add_section("core_instructions", CORE_INSTRUCTIONS, true, 90);
        builder.add_section("tools", tool_descriptions, true, 80);
        let channel_rules = builder.channel_rules_section();
        if !channel_rules.is_empty() {
            builder.add_section("channel_rules", channel_rules, true, 85);
        }
        builder.add_section("git_safety", crate::git_context::GIT_SAFETY_RULES, true, 70);

        // Dynamic sections
        if let Some(soul) = soul_prompt {
            builder.add_section("soul", soul, false, 60);
        }
        if let Some(git) = git_context {
            builder.add_section("git_context", git, false, 50);
        }
        if let Some(mem) = memory {
            builder.add_section("memory", mem, false, 40);
        }
        if let Some(mcp) = mcp_instructions {
            builder.add_section("mcp_instructions", mcp, false, 30);
        }
        if plan_mode_active {
            builder.add_section(
                "plan_mode",
                crate::tools::plan_mode::PLAN_MODE_CONSTRAINT,
                false,
                95, // High priority — must not be compacted away
            );
        }

        builder.build()
    }
}

const IDENTITY_PROMPT: &str = "You are NexiBot, a top-tier autonomous AI agent. You handle complex tasks across coding, research, analysis, communication, and system operations. You are part of the Paperclip ecosystem alongside Animus (persistent AI) and KN-Code (dedicated coding agent). Users choose you for your versatility and reliability.";

const CORE_INSTRUCTIONS: &str = r#"## Core Instructions

- Complete tasks with minimal human intervention unless approval is genuinely needed
- Think step by step for complex problems; explore before editing
- For coding tasks: READ files before editing them; use nexibot_file_edit for targeted changes, not nexibot_file_read + full rewrites
- Prefer small, focused edits over large rewrites
- Verify your changes make sense before finalizing
- Do not add features, comments, or error handling beyond what was asked
- If blocked, explain clearly; don't silently fail"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_simple_prompt() {
        let mut builder = SystemPromptBuilder::new(ChannelContext::Gui);
        builder.add_section("s1", "static content", true, 100);
        builder.add_section("s2", "dynamic content", false, 50);
        let result = builder.build();
        assert!(result.contains("static content"));
        assert!(result.contains("[DYNAMIC_BOUNDARY]"));
        assert!(result.contains("dynamic content"));
        // Boundary should appear before dynamic
        let boundary_pos = result.find("[DYNAMIC_BOUNDARY]").unwrap();
        let dynamic_pos = result.find("dynamic content").unwrap();
        assert!(boundary_pos < dynamic_pos);
    }

    #[test]
    fn test_telegram_channel_rules_html_only() {
        let builder = SystemPromptBuilder::new(ChannelContext::Telegram);
        let rules = builder.channel_rules_section();
        assert!(rules.contains("HTML"));
        assert!(rules.contains("<b>"));
        assert!(!rules.contains("**"));
    }

    #[test]
    fn test_voice_channel_rules_no_markdown() {
        let builder = SystemPromptBuilder::new(ChannelContext::Voice);
        let rules = builder.channel_rules_section();
        assert!(rules.contains("spoken language"));
        assert!(rules.contains("No markdown"));
    }

    #[test]
    fn test_gui_channel_rules_empty() {
        let builder = SystemPromptBuilder::new(ChannelContext::Gui);
        let rules = builder.channel_rules_section();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_build_coding_session_includes_identity() {
        let prompt = SystemPromptBuilder::build_coding_session(
            ChannelContext::Gui,
            None, None, None, None, false, "tool: foo"
        );
        assert!(prompt.contains("NexiBot"));
        assert!(prompt.contains("tool: foo"));
    }

    #[test]
    fn test_build_coding_session_plan_mode_adds_constraint() {
        let prompt = SystemPromptBuilder::build_coding_session(
            ChannelContext::Gui,
            None, None, None, None, true, ""
        );
        assert!(prompt.contains("PLAN MODE ACTIVE"));
    }

    #[test]
    fn test_skip_empty_sections() {
        let mut builder = SystemPromptBuilder::new(ChannelContext::Gui);
        builder.add_section("empty", "", true, 100);
        builder.add_section("content", "hello", true, 90);
        let result = builder.build();
        // The empty section should not produce any output or blank separators
        assert_eq!(result.trim(), "hello");
        // No double-blank artifacts from the skipped section
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn test_no_boundary_when_all_sections_cacheable() {
        let mut builder = SystemPromptBuilder::new(ChannelContext::Gui);
        builder.add_section("s1", "first", true, 100);
        builder.add_section("s2", "second", true, 90);
        let result = builder.build();
        assert!(!result.contains("[DYNAMIC_BOUNDARY]"));
        assert!(result.contains("first"));
        assert!(result.contains("second"));
    }

    #[test]
    fn test_boundary_first_when_all_sections_dynamic() {
        let mut builder = SystemPromptBuilder::new(ChannelContext::Gui);
        builder.add_section("d1", "dynamic one", false, 50);
        builder.add_section("d2", "dynamic two", false, 40);
        let result = builder.build();
        assert!(result.contains("[DYNAMIC_BOUNDARY]"));
        // Boundary should appear before both dynamic sections
        let boundary_pos = result.find("[DYNAMIC_BOUNDARY]").unwrap();
        let d1_pos = result.find("dynamic one").unwrap();
        assert!(boundary_pos < d1_pos);
    }
}
