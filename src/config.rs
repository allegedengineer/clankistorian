use std::path::PathBuf;

use glob::Pattern;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, clap::Parser)]
pub struct Config {
	/// OpenAI-compatible API endpoint.
	#[arg(
		long,
		env = "CLANKISTORIAN_API",
		default_value = "http://127.0.0.1:11434/v1"
	)]
	pub endpoint: String,
	/// API token.
	#[arg(long, env = "CLANKISTORIAN_TOKEN")]
	pub token: Option<String>,
	/// AI model ID.
	#[arg(long, env = "CLANKISTORIAN_MODEL", default_value = "lfm3")]
	pub model: String,
	/// AI model context window.
	#[arg(long, env = "CLANKISTORIAN_CTX")]
	pub context: Option<u32>,
	/// Should the AI try to reason?
	#[arg(long)]
	pub think: bool,
	/// Prefix automatically appended to automatic commits.
	#[arg(long, default_value = "auto: ", env = "CLANKISTORIAN_PREFIX")]
	pub prefix: String,
	/// File from which to read the prompt.
	#[arg(long, env = "CLANKISTORIAN_PROMPT_FILE")]
	pub prompt_file: Option<PathBuf>,
	/// Globs to ignore in diffs.
	#[arg(long = "silence-file", env = "CLANKISTORIAN_SILENT_FILES")]
	pub silent_files: Vec<String>,
	/// Globs to strip from the diff to save context.
	#[arg(long = "noisy-file", env = "CLANKISTORIAN_NOISY_FILES")]
	pub noisy_files: Vec<String>,
	/// Timer (in seconds) that starts on the most recent change.
	/// When it ends, we commit and start writing a commit message.
	#[arg(long, default_value = "30", env = "CLANKISTORIAN_WAIT_BEFORE_COMMIT")]
	pub wait_before_commit: f32,
	/// Timer (in seconds) that starts when we commit and ends on the first token.
	/// When it ends, we time out the model.
	#[arg(long, default_value = "30", env = "CLANKISTORIAN_WAIT_TIMEOUT")]
	pub wait_timeout: f32,
	/// Timer (in seconds) that starts on the first token and ends on the last token.
	/// When it ends, we time out the model.
	#[arg(long, default_value = "300", env = "CLANKISTORIAN_WRITE_TIMEOUT")]
	pub write_timeout: f32,
}
