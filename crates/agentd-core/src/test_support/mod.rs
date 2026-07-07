//! In-memory fakes for the `ports::` traits. Compiled only under the
//! `test-support` feature (or `cfg(test)`), never in a release binary. Used by
//! agentd-core's own integration tests and, via a `test-support` dev-dependency,
//! by other crates' tests.

pub mod fake_backend;
pub mod fixed_clock;
pub mod in_memory_store;
pub mod mempal_stub;
pub mod recording_command_runner;

pub use fake_backend::FakeBackend;
pub use fixed_clock::FixedClock;
pub use in_memory_store::InMemoryStore;
pub use mempal_stub::MempalStub;
pub use recording_command_runner::{RecordedCall, RecordingCommandRunner};
