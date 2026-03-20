//! Agent engine — workflow spec types, local capability dispatch, and LLM-based planning.
//!
//! This module bridges local DAG execution and the backend agent-engine service,
//! using a shared WorkflowSpec JSON format.

pub mod capability_dispatch;
pub mod planner;
pub mod workflow_executor;
pub mod workflow_spec;
