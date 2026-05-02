use serde::{Serialize, de::DeserializeOwned};

/// A domain event that can be persisted to the event store.
///
/// Event payloads are stored as JSON. The `kind` string is *also* indexed as
/// its own column in the event store so we can filter by event type without
/// parsing every payload. The kind must round-trip through serde so the same
/// string used for dispatch matches what the JSON payload contains under its
/// discriminator tag.
pub trait DomainEvent: Serialize + DeserializeOwned + Clone {
    fn kind(&self) -> &'static str;
}

/// An event-sourced aggregate: a type that turns commands into events and
/// folds events into state.
///
/// Both `decide` and `apply` are pure. `decide` returns the events produced by
/// a command; `apply` mutates the in-memory state in place so replaying a long
/// history doesn't require allocating a new state per step.
///
/// The aggregate type itself is a marker (usually a unit struct). State lives
/// in `Self::State` and has a meaningful `Default` — an "empty" state on which
/// a creation event is valid and all other events are not.
pub trait Aggregate: Sized {
    /// Identifier for this aggregate in the event store's `aggregate` column.
    /// Examples: `"project"`, `"worktree"`, `"session"`, `"turn"`.
    const KIND: &'static str;

    type Command;
    type Event: DomainEvent;
    type State: Default + Clone;
    type Error: std::error::Error + Send + Sync + 'static;

    fn decide(state: &Self::State, cmd: Self::Command) -> Result<Vec<Self::Event>, Self::Error>;

    fn apply(state: &mut Self::State, event: &Self::Event);
}

/// Replay a slice of events into a fresh state. Useful for rehydrating an
/// aggregate from the event store.
pub fn replay<A: Aggregate>(events: &[A::Event]) -> A::State {
    let mut state = A::State::default();
    for event in events {
        A::apply(&mut state, event);
    }
    state
}
