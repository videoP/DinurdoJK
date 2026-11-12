//! Local language models for the chat features (translation, the chat filter
//! and chat summaries). Models are GGUF files fetched once from an address the
//! user chooses, cached on disk and run in-process on the CPU with llama.cpp.
//! Chat text handled here never leaves the machine.

pub mod download;
mod engine;
pub mod template;

pub use engine::{default_threads, engine, Engine, Generate, ModelState, Priority, Sampling};

/// Default translation model: Tencent Hy-MT2 1.8B (Apache-2.0), a dedicated
/// translation model covering 33 languages. About 1.1 GB on disk and in memory.
pub const DEFAULT_TRANSLATE_URL: &str =
    "https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q4_K_M.gguf";

/// Default general model for the chat filter and summaries: Qwen3.5 4B
/// (Apache-2.0), multilingual. About 2.7 GB on disk and in memory, and about
/// a second per chat line on a desktop CPU. Scored on real chat lines it
/// tells banter from abuse and follows the strictness setting; the 2B model
/// below does neither reliably.
pub const DEFAULT_ASSIST_URL: &str = "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/main/Qwen3.5-4B-Q4_K_M.gguf";

/// Lighter alternative: Qwen3.5 2B, about 1.3 GB and half the time per line.
/// Catches explicit words in any spelling, but misjudges banter and barely
/// reacts to the strictness setting.
pub const LIGHT_ASSIST_URL: &str = "https://huggingface.co/unsloth/Qwen3.5-2B-GGUF/resolve/main/Qwen3.5-2B-Q4_K_M.gguf";
