//! Shared domain types for Oxyris (events, commands, IDs, environment).
//!
//! This crate defines the event-sourcing primitives — `Aggregate`,
//! `DomainEvent`, `StoredEvent` — that `apps/desktop` uses to build its
//! domain layer. Concrete aggregates (`Project`, `Worktree`, `Session`,
//! `Turn`) live in `apps/desktop/src/domain/`.

#![forbid(unsafe_code)]

pub mod aggregate;
pub mod environment;
pub mod event;
pub mod ids;

pub use aggregate::{Aggregate, DomainEvent, replay};
pub use environment::Environment;
pub use event::StoredEvent;
pub use ids::AggregateId;
