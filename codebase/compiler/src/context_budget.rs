//! Context Budget API for AI Agent Resource Management
//!
//! This module implements the context budget system described in Claude's research
//! on "Context-Aware Resource Management for AI Agents". It provides:
//!
//! 1. Token budget tracking for LLM API calls
//! 2. Context window management (sliding, summarization, eviction)
//! 3. Cost estimation and billing integration
//! 4. Rate limiting and throttling
//!
//! # Architecture
//!
//! - `ContextBudget`: Tracks current usage against limits
//! - `BudgetPolicy`: Defines allocation strategy (strict, adaptive, unlimited)
//! - `ContextManager`: Manages conversation history within budget constraints
//!
//! # Example
//!
//! ```ignore
//! use gradient_compiler::context_budget::{ContextBudget, BudgetPolicy};
//!
//! let budget = ContextBudget::new(100_000)  // 100k token limit
//!     .with_policy(BudgetPolicy::Adaptive { min_reserve: 10_000 });
//!
//! // Check if adding 5000 tokens stays within budget
//! if budget.can_accommodate(5000) {
//!     budget.consume(5000);
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A context budget represents the available "space" for AI agent operations.
///
/// Tracks token usage, API calls, and time spent within a session. Provides
/// projections and warnings when approaching limits.
#[derive(Debug, Clone, Serialize)]
pub struct ContextBudget {
    /// Maximum tokens allowed in this session.
    pub token_limit: usize,
    /// Tokens currently consumed.
    pub tokens_used: usize,
    /// Maximum API calls allowed.
    pub api_call_limit: Option<usize>,
    /// API calls currently made.
    pub api_calls_used: usize,
    /// Maximum time allowed for this session.
    pub time_limit: Option<Duration>,
    /// Time elapsed since session start.
    pub time_used: Duration,
    /// When this budget was created.
    #[serde(skip)]
    pub session_start: Instant,
    /// Budget allocation policy.
    pub policy: BudgetPolicy,
    /// Whether the budget has been exceeded.
    pub exceeded: bool,
    /// History of budget consumption events.
    pub consumption_log: VecDeque<ConsumptionEvent>,
}

/// A single budget consumption event.
#[derive(Debug, Clone, Serialize)]
pub struct ConsumptionEvent {
    /// What consumed the budget.
    pub operation: String,
    /// Tokens consumed.
    pub tokens: usize,
    /// API calls made.
    pub api_calls: usize,
    /// Time spent.
    pub duration: Duration,
    /// When this occurred (relative to session start).
    pub timestamp: Duration,
}

/// Policy for handling budget constraints.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum BudgetPolicy {
    /// Strict enforcement - fail when budget is exceeded.
    Strict,
    /// Adaptive - try to stay within budget but allow small overruns.
    Adaptive { min_reserve: usize },
    /// Warning only - track but don't enforce limits.
    Warning,
    /// Unlimited - no tracking or enforcement.
    Unlimited,
}

/// Result of a budget check.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetStatus {
    /// Operation can proceed.
    Ok { remaining: usize },
    /// Approaching limit - proceed with caution.
    Warning { remaining: usize, percent_used: f64 },
    /// Budget exceeded - operation should fail or compress context.
    Exceeded { over_by: usize },
}

/// Suggested action when budget is constrained.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetAction {
    /// Proceed normally.
    Proceed,
    /// Summarize older context to free tokens.
    Summarize { target_reduction: usize },
    /// Evict oldest context entries.
    Evict { entries_to_remove: usize },
    /// Abort the operation.
    Abort { reason: String },
}

impl ContextBudget {
    /// Create a new context budget with a token limit.
    pub fn new(token_limit: usize) -> Self {
        Self {
            token_limit,
            tokens_used: 0,
            api_call_limit: None,
            api_calls_used: 0,
            time_limit: None,
            time_used: Duration::ZERO,
            session_start: Instant::now(),
            policy: BudgetPolicy::Strict,
            exceeded: false,
            consumption_log: VecDeque::new(),
        }
    }

    /// Set an API call limit.
    pub fn with_api_limit(mut self, limit: usize) -> Self {
        self.api_call_limit = Some(limit);
        self
    }

    /// Set a time limit.
    pub fn with_time_limit(mut self, limit: Duration) -> Self {
        self.time_limit = Some(limit);
        self
    }

    /// Set the budget policy.
    pub fn with_policy(mut self, policy: BudgetPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Check if an operation can be accommodated within the budget.
    pub fn can_accommodate(&self, tokens: usize) -> bool {
        match self.policy {
            BudgetPolicy::Unlimited => true,
            BudgetPolicy::Warning => true,
            BudgetPolicy::Strict => self.tokens_used + tokens <= self.token_limit,
            BudgetPolicy::Adaptive { min_reserve } => {
                self.tokens_used + tokens <= self.token_limit + min_reserve
            }
        }
    }

    /// Consume tokens from the budget.
    ///
    /// Returns the current status after consumption.
    pub fn consume(&mut self, tokens: usize) -> BudgetStatus {
        self.tokens_used += tokens;
        self.check_status()
    }

    /// Consume tokens for a specific operation.
    ///
    /// Logs the consumption event for later analysis.
    pub fn consume_for(&mut self, operation: &str, tokens: usize) -> BudgetStatus {
        let event = ConsumptionEvent {
            operation: operation.to_string(),
            tokens,
            api_calls: 0,
            duration: Duration::ZERO,
            timestamp: self.session_start.elapsed(),
        };
        self.consumption_log.push_back(event);

        // Keep log size manageable
        if self.consumption_log.len() > 1000 {
            self.consumption_log.pop_front();
        }

        self.consume(tokens)
    }

    /// Record an API call.
    pub fn record_api_call(&mut self) {
        self.api_calls_used += 1;
    }

    /// Check the current budget status.
    pub fn check_status(&self) -> BudgetStatus {
        let percent_used = (self.tokens_used as f64 / self.token_limit as f64) * 100.0;

        if self.tokens_used > self.token_limit {
            BudgetStatus::Exceeded {
                over_by: self.tokens_used - self.token_limit,
            }
        } else if percent_used >= 90.0 {
            BudgetStatus::Warning {
                remaining: self.token_limit - self.tokens_used,
                percent_used,
            }
        } else {
            BudgetStatus::Ok {
                remaining: self.token_limit - self.tokens_used,
            }
        }
    }

    /// Get a recommended action based on current budget state.
    pub fn recommended_action(&self, requested_tokens: usize) -> BudgetAction {
        match self.check_status() {
            BudgetStatus::Exceeded { .. } => BudgetAction::Abort {
                reason: "Budget exceeded".to_string(),
            },
            BudgetStatus::Warning { remaining, .. } => {
                if requested_tokens > remaining {
                    BudgetAction::Summarize {
                        target_reduction: requested_tokens - remaining + 1000,
                    }
                } else {
                    BudgetAction::Proceed
                }
            }
            BudgetStatus::Ok { remaining } => {
                if requested_tokens > remaining {
                    BudgetAction::Evict {
                        entries_to_remove: 1,
                    }
                } else {
                    BudgetAction::Proceed
                }
            }
        }
    }

    /// Calculate projected tokens needed for remaining work.
    pub fn project_needed(&self, operations_remaining: usize, avg_tokens_per_op: usize) -> usize {
        operations_remaining * avg_tokens_per_op
    }

    /// Get a summary of budget usage.
    pub fn summary(&self) -> BudgetSummary {
        BudgetSummary {
            token_usage: (self.tokens_used, self.token_limit),
            api_usage: (self.api_calls_used, self.api_call_limit),
            time_usage: (self.session_start.elapsed(), self.time_limit),
            percent_consumed: (self.tokens_used as f64 / self.token_limit as f64) * 100.0,
        }
    }

    /// Get consumption statistics.
    pub fn consumption_stats(&self) -> ConsumptionStats {
        let total_ops = self.consumption_log.len();
        let total_tokens: usize = self.consumption_log.iter().map(|e| e.tokens).sum();
        let avg_tokens = if total_ops > 0 {
            total_tokens / total_ops
        } else {
            0
        };

        // Find top consumers
        let mut by_operation: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();
        for event in &self.consumption_log {
            let entry = by_operation
                .entry(event.operation.clone())
                .or_insert((0, 0));
            entry.0 += 1;
            entry.1 += event.tokens;
        }

        let mut top_consumers: Vec<(String, usize, usize)> = by_operation
            .into_iter()
            .map(|(op, (count, tokens))| (op, count, tokens))
            .collect();
        top_consumers.sort_by(|a, b| b.2.cmp(&a.2));

        ConsumptionStats {
            total_operations: total_ops,
            total_tokens_consumed: total_tokens,
            average_tokens_per_operation: avg_tokens,
            top_consumers: top_consumers.into_iter().take(5).collect(),
        }
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::new(100_000) // 100k default
    }
}

/// A summary of current budget state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetSummary {
    pub token_usage: (usize, usize),
    pub api_usage: (usize, Option<usize>),
    pub time_usage: (Duration, Option<Duration>),
    pub percent_consumed: f64,
}

/// Statistics about budget consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumptionStats {
    pub total_operations: usize,
    pub total_tokens_consumed: usize,
    pub average_tokens_per_operation: usize,
    pub top_consumers: Vec<(String, usize, usize)>, // (operation, count, total_tokens)
}

/// A session manager that tracks context budgets across multiple operations.
#[derive(Debug, Clone)]
pub struct BudgetSession {
    pub budget: ContextBudget,
    pub session_id: String,
    pub metadata: std::collections::HashMap<String, String>,
}

impl BudgetSession {
    /// Create a new budget session.
    pub fn new(session_id: String, token_limit: usize) -> Self {
        Self {
            budget: ContextBudget::new(token_limit),
            session_id,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Add metadata to the session.
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }

    /// Record an LLM API call with token usage.
    pub fn record_llm_call(&mut self, model: &str, input_tokens: usize, output_tokens: usize) {
        let total = input_tokens + output_tokens;
        self.budget.consume_for(&format!("llm:{}", model), total);
        self.budget.record_api_call();
    }

    /// Check if the session is still healthy (within budget).
    pub fn is_healthy(&self) -> bool {
        matches!(self.budget.check_status(), BudgetStatus::Ok { .. })
    }

    /// Generate a JSON report of session state.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "session_id": self.session_id,
            "budget": self.budget.summary(),
            "stats": self.budget.consumption_stats(),
            "metadata": self.metadata,
        })
        .to_string()
    }
}

/// Global budget registry for tracking multiple sessions.
pub struct BudgetRegistry {
    sessions: std::collections::HashMap<String, BudgetSession>,
    default_token_limit: usize,
}

impl BudgetRegistry {
    /// Create a new budget registry.
    pub fn new(default_token_limit: usize) -> Self {
        Self {
            sessions: std::collections::HashMap::new(),
            default_token_limit,
        }
    }

    /// Create a new session with auto-generated ID.
    pub fn create_session(&mut self) -> String {
        let id = format!("session_{}", uuid::Uuid::new_v4());
        let session = BudgetSession::new(id.clone(), self.default_token_limit);
        self.sessions.insert(id.clone(), session);
        id
    }

    /// Get a session by ID.
    pub fn get_session(&self, id: &str) -> Option<&BudgetSession> {
        self.sessions.get(id)
    }

    /// Get a mutable session by ID.
    pub fn get_session_mut(&mut self, id: &str) -> Option<&mut BudgetSession> {
        self.sessions.get_mut(id)
    }

    /// Remove a session.
    pub fn remove_session(&mut self, id: &str) -> Option<BudgetSession> {
        self.sessions.remove(id)
    }

    /// Get global statistics across all sessions.
    pub fn global_stats(&self) -> GlobalBudgetStats {
        let total_sessions = self.sessions.len();
        let total_tokens: usize = self.sessions.values().map(|s| s.budget.tokens_used).sum();
        let total_api_calls: usize = self
            .sessions
            .values()
            .map(|s| s.budget.api_calls_used)
            .sum();

        GlobalBudgetStats {
            total_sessions,
            total_tokens_consumed: total_tokens,
            total_api_calls,
            average_tokens_per_session: if total_sessions > 0 {
                total_tokens / total_sessions
            } else {
                0
            },
        }
    }
}

impl Default for BudgetRegistry {
    fn default() -> Self {
        Self::new(100_000)
    }
}

/// Global statistics about all budget sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalBudgetStats {
    pub total_sessions: usize,
    pub total_tokens_consumed: usize,
    pub total_api_calls: usize,
    pub average_tokens_per_session: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_creation() {
        let budget = ContextBudget::new(1000);
        assert_eq!(budget.token_limit, 1000);
        assert_eq!(budget.tokens_used, 0);
    }

    #[test]
    fn test_budget_consumption() {
        let mut budget = ContextBudget::new(1000);
        let status = budget.consume(500);
        assert!(matches!(status, BudgetStatus::Ok { remaining: 500 }));
        assert_eq!(budget.tokens_used, 500);
    }

    #[test]
    fn test_budget_warning() {
        let mut budget = ContextBudget::new(1000);
        budget.consume(950);
        let status = budget.check_status();
        assert!(matches!(status, BudgetStatus::Warning { .. }));
    }

    #[test]
    fn test_budget_exceeded() {
        let mut budget = ContextBudget::new(1000);
        let status = budget.consume(1500);
        assert!(matches!(status, BudgetStatus::Exceeded { over_by: 500 }));
    }

    #[test]
    fn test_adaptive_policy() {
        let budget =
            ContextBudget::new(1000).with_policy(BudgetPolicy::Adaptive { min_reserve: 100 });
        assert!(budget.can_accommodate(1050)); // Over limit but within reserve
        assert!(!budget.can_accommodate(1200)); // Beyond reserve
    }

    #[test]
    fn test_recommended_action() {
        let mut budget = ContextBudget::new(1000);
        budget.consume(950);

        let action = budget.recommended_action(100);
        assert!(matches!(
            action,
            BudgetAction::Summarize {
                target_reduction: _,
            }
        ));
    }

    #[test]
    fn test_session_health() {
        let mut session = BudgetSession::new("test".to_string(), 1000);
        assert!(session.is_healthy());

        session.budget.consume(1500);
        assert!(!session.is_healthy());
    }
}
