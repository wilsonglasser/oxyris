//! Auto-pilot decision policy — the deterministic safety pipeline wrapped around
//! a [`Supervisor`]. The desktop `AutopilotController` owns one [`Autopilot`] per
//! engaged session, feeds it detected prompts/turn-ends, and carries out the
//! returned [`Action`] (PTY stdin for pure, approve/reject/reply for structured).
//!
//! The safety-critical parts ([`Autopilot::pre_check`], [`Autopilot::post_decision`])
//! are pure and unit-tested. Only [`Autopilot::step`] is async — a thin glue that
//! consults the Supervisor between the two pure halves.

use crate::guardrails::{Budget, Denylist, LoopGuard, LoopVerdict};
use crate::{AutopilotContext, Decision, PendingKind, Supervisor, SupervisorError};

/// What the controller should carry out for one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Approve the pending tool / answer "yes".
    Approve,
    /// Approve and prefer a "don't ask again" menu option when present.
    ApproveAlways,
    /// Reject the pending tool; `reason` is fed back to the model.
    Reject(String),
    /// Send a reply / next instruction.
    Reply(String),
    /// Stop the pilot and hand back to the human, with why.
    Halt(HaltReason),
}

/// Why an engaged pilot stopped. All of these surface to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HaltReason {
    /// Supervisor judged the mission complete.
    Done(String),
    /// Supervisor wasn't confident — escalated to the human.
    Escalated(String),
    /// A denylisted (irreversible/dangerous) action was requested.
    Denylisted(String),
    /// Supervisor and claude looped without progress.
    Looping,
    /// The run hit its turn/step budget.
    BudgetExhausted,
}

/// Per-session auto-pilot state machine: mission config + guardrails + the
/// Supervisor backend.
pub struct Autopilot {
    denylist: Denylist,
    loop_guard: LoopGuard,
    budget: Budget,
    supervisor: Box<dyn Supervisor>,
}

impl Autopilot {
    pub fn new(supervisor: Box<dyn Supervisor>, max_turns: Option<u32>) -> Self {
        Self {
            denylist: Denylist::new(),
            loop_guard: LoopGuard::default(),
            budget: Budget::new(max_turns),
            supervisor,
        }
    }

    /// Pure pre-flight: runs the denylist + loop guard *before* the Supervisor is
    /// consulted. Returns `Some(Halt)` to stop immediately, or `None` to proceed
    /// to [`Autopilot::step`]'s Supervisor call.
    pub fn pre_check(&mut self, ask: &PendingKind) -> Option<Action> {
        // Denylist first — a forbidden action never reaches the Supervisor.
        if let Some(text) = ask.approval_text()
            && let Some(name) = self.denylist.first_match(text)
        {
            return Some(Action::Halt(HaltReason::Denylisted(name.to_owned())));
        }
        // Loop / step guard, keyed by what we're acting on.
        let fingerprint = match ask {
            PendingKind::Permission { raw_prompt, .. } => raw_prompt.as_str(),
            PendingKind::TurnEnded { last_output } => last_output.as_str(),
        };
        match self.loop_guard.observe(fingerprint) {
            LoopVerdict::Ok => None,
            LoopVerdict::Repeating => Some(Action::Halt(HaltReason::Looping)),
            LoopVerdict::TooManySteps => Some(Action::Halt(HaltReason::BudgetExhausted)),
        }
    }

    /// Pure post-flight: maps a Supervisor [`Decision`] to an [`Action`], charging
    /// the turn budget when the decision drives a new turn (a reply to a settled
    /// turn). Re-checks the denylist on `Approve` as defence in depth.
    pub fn post_decision(&mut self, ask: &PendingKind, decision: Decision) -> Action {
        match decision {
            Decision::Approve | Decision::ApproveAlways => {
                // Defence in depth: never approve a denylisted action even if the
                // Supervisor said yes.
                if let Some(text) = ask.approval_text()
                    && let Some(name) = self.denylist.first_match(text)
                {
                    return Action::Halt(HaltReason::Denylisted(name.to_owned()));
                }
                if matches!(decision, Decision::ApproveAlways) {
                    Action::ApproveAlways
                } else {
                    Action::Approve
                }
            }
            Decision::Reject { reason } => Action::Reject(reason),
            Decision::Reply { text } => {
                // A reply to a settled turn starts a new driven turn — charge it.
                if matches!(ask, PendingKind::TurnEnded { .. }) && !self.budget.record_turn() {
                    return Action::Halt(HaltReason::BudgetExhausted);
                }
                Action::Reply(text)
            }
            Decision::Done { summary } => Action::Halt(HaltReason::Done(summary)),
            Decision::Escalate { why } => Action::Halt(HaltReason::Escalated(why)),
        }
    }

    /// Full step: pre-check → Supervisor → post-decision. The only async path.
    /// Returns the [`Action`] to carry out plus the Supervisor's optional one-line
    /// rationale (surfaced as the pilot's "thinking"). A pre-check halt has no
    /// rationale.
    pub async fn step(
        &mut self,
        ctx: &AutopilotContext,
        ask: &PendingKind,
    ) -> Result<(Action, Option<String>), SupervisorError> {
        if let Some(halt) = self.pre_check(ask) {
            return Ok((halt, None));
        }
        let verdict = self.supervisor.decide(ctx, ask).await?;
        Ok((self.post_decision(ask, verdict.decision), verdict.reasoning))
    }

    pub fn turns_used(&self) -> u32 {
        self.budget.turns()
    }

    pub fn steps_taken(&self) -> usize {
        self.loop_guard.steps()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Verdict;
    use async_trait::async_trait;

    /// A Supervisor that always returns a fixed decision — lets the pure policy
    /// be exercised without a real LLM.
    struct FakeSupervisor(Decision);

    #[async_trait]
    impl Supervisor for FakeSupervisor {
        fn id(&self) -> &'static str {
            "fake"
        }
        async fn decide(
            &self,
            _ctx: &AutopilotContext,
            _ask: &PendingKind,
        ) -> Result<Verdict, SupervisorError> {
            Ok(Verdict::bare(self.0.clone()))
        }
    }

    fn perm(prompt: &str, command: Option<&str>) -> PendingKind {
        PendingKind::Permission {
            request_id: None,
            tool_name: Some("Bash".into()),
            command: command.map(str::to_owned),
            raw_prompt: prompt.into(),
        }
    }

    #[test]
    fn pre_check_halts_on_denylisted_command() {
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), None);
        let ask = perm("Run command?", Some("rm -rf /"));
        assert_eq!(
            a.pre_check(&ask),
            Some(Action::Halt(HaltReason::Denylisted(
                "recursive force remove".to_owned()
            )))
        );
    }

    #[test]
    fn pre_check_passes_safe_command() {
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), None);
        let ask = perm("Run command?", Some("cargo test"));
        assert_eq!(a.pre_check(&ask), None);
    }

    #[test]
    fn pre_check_halts_on_loop() {
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), None);
        let ask = perm("Do you want to proceed?", None);
        assert_eq!(a.pre_check(&ask), None);
        assert_eq!(a.pre_check(&ask), None);
        // Third identical prompt → looping (default repeat_limit 3).
        assert_eq!(a.pre_check(&ask), Some(Action::Halt(HaltReason::Looping)));
    }

    #[test]
    fn post_decision_blocks_approve_of_denylisted() {
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), None);
        let ask = perm("Run?", Some("git push --force"));
        // Even with an Approve decision, denylist wins.
        let action = a.post_decision(&ask, Decision::Approve);
        assert_eq!(
            action,
            Action::Halt(HaltReason::Denylisted("force push".to_owned()))
        );
    }

    #[test]
    fn post_decision_charges_budget_on_turn_reply() {
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), Some(1));
        let ask = PendingKind::TurnEnded {
            last_output: "done".into(),
        };
        // First reply consumes the only turn → budget exhausted.
        let action = a.post_decision(
            &ask,
            Decision::Reply {
                text: "next".into(),
            },
        );
        assert_eq!(action, Action::Halt(HaltReason::BudgetExhausted));
    }

    #[test]
    fn post_decision_reply_to_prompt_does_not_charge_budget() {
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), Some(1));
        let ask = perm("Which approach?", None);
        let action = a.post_decision(
            &ask,
            Decision::Reply {
                text: "option 1".into(),
            },
        );
        assert_eq!(action, Action::Reply("option 1".to_owned()));
        assert_eq!(a.turns_used(), 0);
    }

    #[tokio::test]
    async fn step_approves_safe_tool() {
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), None);
        let ctx = AutopilotContext {
            mission: crate::Mission::new("ship it"),
            transcript: Default::default(),
            cwd: "/proj".into(),
        };
        let ask = perm("Run cargo build?", Some("cargo build"));
        assert_eq!(a.step(&ctx, &ask).await.unwrap().0, Action::Approve);
    }

    #[tokio::test]
    async fn step_halts_denylisted_without_consulting_supervisor() {
        // Supervisor would Approve, but the denylist short-circuits in pre_check.
        let mut a = Autopilot::new(Box::new(FakeSupervisor(Decision::Approve)), None);
        let ctx = AutopilotContext {
            mission: crate::Mission::new("ship it"),
            transcript: Default::default(),
            cwd: "/proj".into(),
        };
        let ask = perm("Run?", Some("sudo rm -rf /var"));
        assert_eq!(
            a.step(&ctx, &ask).await.unwrap().0,
            Action::Halt(HaltReason::Denylisted("recursive force remove".to_owned()))
        );
    }
}
