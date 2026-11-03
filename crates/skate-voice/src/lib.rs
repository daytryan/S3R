//! Online voice chat without Bevy: the microphone, noise suppression, Opus, the jitter buffer and the mix, all on the
//! voice thread (docs/superpowers/plans/2026-10-26-voice-chat.md; spec docs/superpowers/specs/2026-10-26-voice-chat-design.md).

pub mod codec;
pub mod device;
pub mod dsp;
pub mod engine;
pub mod jitter;
pub mod mix;
pub mod priority;

/// Whether High noise suppression (DeepFilterNet) needs FMA to keep up: no. On tract's generic kernels, what a CPU
/// without AVX2/FMA runs, it took 1.1 ms a 10 ms frame (worst 3.1 ms) on a Ryzen 5 7535HS, inside the 4 ms budget with
/// room for a CPU half as fast (DECISIONS.md).
pub const HIGH_NEEDS_FMA: bool = false;
