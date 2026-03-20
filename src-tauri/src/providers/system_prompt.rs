//! System prompt building logic extracted from claude.rs.
//!
//! Provides functions to build the system prompt with SOUL context,
//! skills context, memory context, capabilities, and permissions.
#![allow(dead_code)]

use crate::channel::ChannelSource;
use crate::config::{AutonomyLevel, NexiBotConfig};
use crate::memory::MemoryManager;
use crate::skills::SkillsManager;
use crate::soul::Soul;

/// System prompt suffix appended for voice interactions so Claude responds
/// in a way that sounds natural when read aloud by TTS.
const VOICE_MODE_PROMPT: &str = "\n\n\
## Voice Conversation Mode\n\
You are in a live voice conversation. Your response will be spoken aloud via TTS.\n\
\n\
Rules:\n\
- NO markdown: no asterisks, backticks, headers, bullets, numbered lists, or links.\n\
- Talk like a knowledgeable friend. Be concise and natural.\n\
- Never include URLs, file paths, or code snippets — they sound terrible spoken aloud.\n\
- If you have tools available, USE them silently to fulfill the request.\n\
  Do NOT describe what tools you would use. Just do it, then report results.\n\
- After using tools, give a brief spoken summary of what you did and found.\n\
- When writing documents or files, write the FULL content to disk, but only \n\
  SPEAK a brief summary (2-3 sentences about key findings).\n\
- If a task will take time, acknowledge it: 'Let me look into that for you.'\n\
- Only say what's worth hearing. Skip meta-commentary about your process.\n\
- If you don't know, say so briefly. Don't speculate at length.";

/// Returns the current date/time and environment context for injection into system prompts.
pub fn current_datetime_context() -> String {
    let now = chrono::Local::now();
    let home = dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let mut workspaces = Vec::new();
    if let Some(home_dir) = dirs::home_dir() {
        for candidate in &[
            "gitrepos",
            "projects",
            "repos",
            "code",
            "dev",
            "workspace",
            "src",
        ] {
            let path = home_dir.join(candidate);
            if path.is_dir() {
                workspaces.push(path.to_string_lossy().to_string());
            }
        }
    }

    let mut ctx = format!(
        "Current date and time: {} ({})\nHome directory: {}",
        now.format("%A, %B %-d, %Y at %-I:%M %p"),
        now.format("%Z"),
        home,
    );

    if !workspaces.is_empty() {
        ctx.push_str(&format!(
            "\nWorkspace directories: {}",
            workspaces.join(", ")
        ));
    }
    ctx.push_str(&format!("\nDocuments: {}/Documents", home));

    ctx
}

/// Build the full system prompt from SOUL, Skills, Memory, config prompt, and channel.
pub fn build_system_prompt(config_system_prompt: &str, channel: Option<&ChannelSource>) -> String {
    let mut system_prompt = String::new();

    if let Ok(soul) = Soul::load() {
        let ctx = soul.get_system_prompt_context();
        if !ctx.is_empty() {
            system_prompt.push_str(&ctx);
            system_prompt.push_str("\n\n");
        }
    }

    if let Ok(manager) = SkillsManager::new() {
        let ctx = manager.get_skills_context();
        if !ctx.is_empty() {
            system_prompt.push_str(&ctx);
            system_prompt.push_str("\n\n");
        }
    }

    if let Ok(manager) = MemoryManager::new() {
        let ctx = manager.get_memory_context(20);
        if !ctx.is_empty() {
            system_prompt.push_str(&ctx);
            system_prompt.push_str("\n\n");
        }
    }

    system_prompt.push_str(&current_datetime_context());
    system_prompt.push_str("\n\n");

    system_prompt.push_str(config_system_prompt);

    if let Some(ChannelSource::Voice) = channel {
        system_prompt.push_str(VOICE_MODE_PROMPT);
    }

    system_prompt
}

/// Build a dynamic capabilities context based on what's enabled in config.
pub fn build_capabilities_context(config: &NexiBotConfig) -> String {
    let mut caps = Vec::new();

    if config.scheduled_tasks.enabled {
        let task_count = config.scheduled_tasks.tasks.len();
        caps.push(format!(
            "- **Scheduler**: You have a built-in task scheduler that runs in the background. \
             You can schedule recurring tasks using formats like \"daily HH:MM\", \"hourly\", \
             \"every Nm\", or \"weekly DAY HH:MM\". Tasks execute your prompts automatically. \
             Currently {} task(s) configured. Users can manage tasks in Settings > Scheduler.",
            task_count
        ));
    } else {
        caps.push(
            "- **Scheduler**: You have a built-in task scheduler (currently disabled). \
             Users can enable it in Settings > Scheduler. It supports daily, hourly, \
             weekly, and interval-based recurring tasks that run your prompts automatically."
                .to_string(),
        );
    }

    if config.k2k.enabled {
        caps.push(format!(
            "- **K2K Local Agent**: Connected to the local Knowledge Nexus System Agent at {}. \
             You can search the user's local indexed files via semantic search.",
            config.k2k.local_agent_url
        ));
        if config.k2k.supermemory_enabled {
            caps.push(
                "- **Supermemory**: Conversations are automatically synced to the System Agent \
                 as persistent long-term memory that survives across sessions."
                    .to_string(),
            );
        }
    }

    if config.mcp.enabled && !config.mcp.servers.is_empty() {
        let server_names: Vec<&str> = config
            .mcp
            .servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.name.as_str())
            .collect();
        if !server_names.is_empty() {
            caps.push(format!(
                "- **MCP Servers**: {} active: {}",
                server_names.len(),
                server_names.join(", ")
            ));
        }
    }

    if config.filesystem.enabled {
        caps.push(
            "- **Filesystem**: You can read and write files on the user's computer.".to_string(),
        );
    }

    if config.execute.enabled {
        caps.push(
            "- **Command Execution**: You can run shell commands on the user's computer."
                .to_string(),
        );
    }

    let has_search =
        config.search.brave_api_key.is_some() || config.search.tavily_api_key.is_some();
    if has_search {
        caps.push("- **Web Search**: You can search the web for current information.".to_string());
    }

    if config.fetch.enabled {
        caps.push("- **Web Fetch**: You can fetch and read web pages.".to_string());
    }

    if config.browser.enabled {
        caps.push("- **Browser Automation**: You can control a headless browser.".to_string());
    }

    if config.audio.enabled {
        caps.push(
            "- **Voice**: Audio input/output is enabled for voice conversations.".to_string(),
        );
    }

    if caps.is_empty() {
        return String::new();
    }

    format!("## Your Capabilities\n\n{}", caps.join("\n"))
}

/// Build a permissions context for the system prompt when autonomous mode is enabled.
pub fn build_permissions_context(config: &NexiBotConfig) -> String {
    if !config.autonomous_mode.enabled {
        return String::new();
    }

    let mut can_do = Vec::new();
    let mut cannot_do = Vec::new();

    // Filesystem
    if config.autonomous_mode.filesystem.read == AutonomyLevel::Autonomous {
        can_do.push("- Read files within allowed paths");
    } else if config.autonomous_mode.filesystem.read == AutonomyLevel::Blocked {
        cannot_do.push("- Read files (disabled by user)");
    }
    if config.autonomous_mode.filesystem.write == AutonomyLevel::Autonomous {
        can_do.push("- Write and create files");
    } else if config.autonomous_mode.filesystem.write == AutonomyLevel::Blocked {
        cannot_do.push("- Write or create files (disabled by user)");
    }
    if config.autonomous_mode.filesystem.delete == AutonomyLevel::Autonomous {
        can_do.push("- Delete files");
    } else if config.autonomous_mode.filesystem.delete == AutonomyLevel::Blocked {
        cannot_do.push("- Delete files (disabled by user)");
    }

    // Execute
    if config.autonomous_mode.execute.run_command == AutonomyLevel::Autonomous {
        can_do.push("- Run shell commands");
    } else if config.autonomous_mode.execute.run_command == AutonomyLevel::Blocked {
        cannot_do.push("- Run shell commands (disabled by user)");
    }
    if config.autonomous_mode.execute.run_python == AutonomyLevel::Autonomous {
        can_do.push("- Run Python scripts");
    } else if config.autonomous_mode.execute.run_python == AutonomyLevel::Blocked {
        cannot_do.push("- Run Python scripts (disabled by user)");
    }
    if config.autonomous_mode.execute.run_node == AutonomyLevel::Autonomous {
        can_do.push("- Run Node.js scripts");
    } else if config.autonomous_mode.execute.run_node == AutonomyLevel::Blocked {
        cannot_do.push("- Run Node.js scripts (disabled by user)");
    }

    // Fetch
    if config.autonomous_mode.fetch.get_requests == AutonomyLevel::Autonomous {
        can_do.push("- Fetch web pages (GET requests)");
    } else if config.autonomous_mode.fetch.get_requests == AutonomyLevel::Blocked {
        cannot_do.push("- Fetch web pages (disabled by user)");
    }
    if config.autonomous_mode.fetch.post_requests == AutonomyLevel::Autonomous {
        can_do.push("- Make POST/PUT/DELETE requests");
    } else if config.autonomous_mode.fetch.post_requests == AutonomyLevel::Blocked {
        cannot_do.push("- Make POST/PUT/DELETE requests (disabled by user)");
    }

    // Browser
    if config.autonomous_mode.browser.navigate == AutonomyLevel::Autonomous {
        can_do.push("- Navigate browser to URLs");
    } else if config.autonomous_mode.browser.navigate == AutonomyLevel::Blocked {
        cannot_do.push("- Navigate browser (disabled by user)");
    }
    if config.autonomous_mode.browser.interact == AutonomyLevel::Autonomous {
        can_do.push("- Interact with browser (click, type, etc.)");
    } else if config.autonomous_mode.browser.interact == AutonomyLevel::Blocked {
        cannot_do.push("- Interact with browser (disabled by user)");
    }

    // Computer use
    if config.autonomous_mode.computer_use.level == AutonomyLevel::Autonomous {
        can_do.push("- Use computer (mouse, keyboard, screenshots)");
    } else if config.autonomous_mode.computer_use.level == AutonomyLevel::Blocked {
        cannot_do.push("- Use computer control (disabled by user)");
    }

    // Self-modification
    if config.autonomous_mode.settings_modification.level == AutonomyLevel::Autonomous {
        can_do.push("- Modify your own settings");
    } else if config.autonomous_mode.settings_modification.level == AutonomyLevel::Blocked {
        cannot_do.push("- Modify settings (disabled by user)");
    }
    if config.autonomous_mode.memory_modification.level == AutonomyLevel::Autonomous {
        can_do.push("- Access and modify memory");
    } else if config.autonomous_mode.memory_modification.level == AutonomyLevel::Blocked {
        cannot_do.push("- Modify memory (disabled by user)");
    }
    if config.autonomous_mode.soul_modification.level == AutonomyLevel::Autonomous {
        can_do.push("- Modify your soul/personality");
    } else if config.autonomous_mode.soul_modification.level == AutonomyLevel::Blocked {
        cannot_do.push("- Modify your soul/personality (disabled by user)");
    }

    let mut sections = Vec::new();
    sections.push("## Your Permissions (Autonomous Mode)\n".to_string());

    if !can_do.is_empty() {
        sections.push("### You CAN do these without asking:".to_string());
        sections.push(can_do.join("\n"));
    }

    if !cannot_do.is_empty() {
        sections.push("\n### You CANNOT do these (refuse and explain if asked):".to_string());
        sections.push(cannot_do.join("\n"));
    }

    sections.push("\n### Hard Safety Limits (always enforced, never attempt):".to_string());
    sections.push("- NEVER execute: rm -rf /, mkfs, fork bombs, dd to raw devices".to_string());
    sections.push(
        "- NEVER access system directories: /etc, /System, /usr, /var, /bin, /sbin".to_string(),
    );
    sections.push(
        "- NEVER expose API keys, passwords, private keys, or credit card numbers".to_string(),
    );
    sections.push("- NEVER modify or delete the config file directly".to_string());

    sections.push("\n### For everything else, proceed with your best judgment.".to_string());

    sections.join("\n")
}
