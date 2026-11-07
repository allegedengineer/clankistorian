use std::{
	env::current_dir,
	fmt::Write,
	path::{Path, PathBuf},
	sync::Arc,
	time::Duration,
};

use async_openai::{
	Chat,
	config::OpenAIConfig,
	types::chat::{
		ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
		ChatCompletionRequestSystemMessageContent, ChatCompletionRequestUserMessage,
		CreateChatCompletionRequest, ReasoningEffort, Verbosity,
	},
};
use futures_util::StreamExt;
use git2::{Delta, DiffOptions, IndexAddOption, Patch, Repository, Signature};
use glob::Pattern;
use notify::{EventKind, INotifyWatcher, RecursiveMode, Watcher, event::ModifyKind};
use tokio::{io::AsyncReadExt, net::UnixListener, select, sync::Notify, time::Instant};
use tracing::{debug, error, info, trace};

use crate::{cmd::Command, config::Config};

pub struct Worker {
	pub config: Config,
	pub state: State,
	pub socket: tokio::net::UnixListener,
	pub repo: Repository,
	pub queue_stop: bool,
	pub paused: bool,
	pub notify: Arc<Notify>,
	pub silence_paths: Arc<[Pattern]>,
	pub noisy_paths: Arc<[Pattern]>,
	pub watch: INotifyWatcher,
}

#[derive(Clone)]
pub enum State {
	/// Ready for a file change or a command.
	Idle,
	/// Waiting for changes to calm down, or for a command.
	Wait,
	/// Make a commit.
	Stage,
	/// Wait for the LLM to write a commit message.
	Generate { diff_txt: String },
	/// Amend the commit message with the LLM's summary.
	Commit { commit_msg: String },
	/// We're ready to finish.
	Done,
}

impl Worker {
	pub async fn new(config: Config, root: PathBuf) -> Self {
		let silence_paths = config
			.silent_files
			.iter()
			.map(|f| Pattern::new(f).expect("Bad glob"))
			.collect::<Arc<[_]>>();
		let noisy_paths = config
			.noisy_files
			.iter()
			.map(|f| Pattern::new(f).expect("Bad glob"))
			.collect::<Arc<[_]>>();
		let notify = Arc::new(tokio::sync::Notify::new());
		let mut watch = notify::recommended_watcher({
			let notify = notify.clone();
			let silence_paths = silence_paths.clone();
			move |e: Result<notify::Event, notify::Error>| {
				let Ok(e) = e else { return };
				if !matches!(e.kind, EventKind::Modify(_) | EventKind::Create(_)) {
					trace!("Unwanted event type.");
					return;
				}
				let paths = e
					.paths
					.iter()
					.filter_map(|p| p.strip_prefix(current_dir().unwrap()).ok())
					.collect::<Vec<_>>();
				if paths.iter().all(|e| {
					!silence_paths.is_empty() && silence_paths.iter().any(|p| p.matches_path(e))
				}) {
					trace!("All files are ignored: {:#?}", paths);
					return;
				}
				info!("Changed: {:#?}", paths);
				notify.notify_one();
			}
		})
		.expect("Failed to start watch");
		watch
			.watch(&root, RecursiveMode::Recursive)
			.expect("Failed to watch");
		let socket_path = root.join(".git/.clankistorian.sock");
		let _ = tokio::fs::remove_file(&socket_path).await;
		Self {
			config,
			repo: Repository::open(&root).expect("Failed to open repo"),
			state: State::Idle,
			socket: UnixListener::bind(&socket_path).expect("Failed to listen"),
			queue_stop: false,
			paused: false,
			notify,
			silence_paths,
			noisy_paths,
			watch,
		}
	}

	pub async fn run(mut self) -> ! {
		loop {
			match &self.state {
				State::Idle => {
					if self.queue_stop {
						info!("Reached idle with queued stop, IDLE->DONE");
						self.state = State::Done;
						continue;
					}
					let timer =
						tokio::time::sleep(Duration::from_secs_f32(self.config.poll_interval));
					let notify = self.notify.notified();
					let command = self.socket.accept();
					tokio::select! {
						_ = timer => self.idle_tick().await,
						_ = notify => self.idle_tick().await,
						connection = command => self.connection(connection.expect("Control socket error").0).await
					}
				}
				State::Wait => {
					let timer =
						tokio::time::sleep(Duration::from_secs_f32(self.config.wait_before_commit));
					let notify = self.notify.notified();
					let command = self.socket.accept();
					tokio::select! {
						_ = timer => self.state = State::Stage,
						_ = notify => {
							info!("Got new changes. Restarting timer.");
							tokio::time::sleep(Duration::from_millis(100)).await;
							continue
						},
						connection = command => self.connection(connection.expect("Control socket error").0).await
					}
				}
				State::Stage => self.stage().await,
				State::Generate { diff_txt } => self.generate(diff_txt.clone()).await,
				State::Commit { commit_msg } => self.commit(commit_msg.clone()).await,
				State::Done => {
					info!("Exiting.");
					std::process::exit(0)
				}
			}
		}
	}

	/// Checks if there are active changes, and switches to `wait` if they are.
	async fn idle_tick(&mut self) {
		info!("Idle tick");
		// Don't do anything if we're paused.
		if self.paused {
			return;
		}
		let mut diff_options = DiffOptions::new();
		diff_options.include_untracked(true)
			.recurse_untracked_dirs(true)
			.show_binary(true);
		let diff = self
			.repo
			.diff_index_to_workdir(None, Some(&mut diff_options))
			.expect("Can't `git diff`");
		let diff_stats = diff.stats().expect("Diff stats");
		info!("Diff: {:#?}", diff.stats());
		if diff.deltas().all(|d| {
			let Some(path) = d.new_file().path().or(d.old_file().path()) else {
				return true;
			};
			!self.silence_paths.is_empty()
				&& self.silence_paths.iter().any(|s| s.matches_path(path))
		}) {
			info!("All affected files are ignored.");
			return;
		}
		if diff_stats.files_changed() > 0 {
			info!(
				"Got new changes. (+{}/-{}), ->WAIT",
				diff_stats.insertions(),
				diff_stats.deletions()
			);
			self.state = State::Wait;
		}
	}

	/// Command socket connection.
	async fn connection(&mut self, mut connection: tokio::net::UnixStream) {
		let mut data = String::new();
		let Ok(_) = connection.read_to_string(&mut data).await else {
			error!("Failed reading command socket.");
			return;
		};
		let Ok(command): Result<Command, _> = serde_json::from_str(&data) else {
			error!("Malformed command {data}");
			return;
		};
		match (command, &self.state) {
			(Command::Stop, State::Idle) => {
				info!("Got stop, IDLE->DONE");
				self.state = State::Done
			}
			(Command::Stop, _) => self.queue_stop = true,
			(Command::Pause, _) => {
				info!("Got pause.");
				self.paused = true
			}
			(Command::Resume, _) => {
				info!("Got resume.");
				self.paused = false
			}
			(Command::Force, State::Idle | State::Wait) => {
				info!("Got force. ->COMMIT");
				self.state = State::Stage
			}
			(Command::Force, _) => {
				info!("Got force, but we're already busy. (No-op.)");
			}
		}
	}

	async fn stage(&mut self) {
		info!("Stage");
		let mut diff_options = DiffOptions::new();
		diff_options
			.include_untracked(true)
			.recurse_untracked_dirs(true)
			.patience(true)
			.minimal(true);
		let diff = self
			.repo
			.diff_index_to_workdir(None, Some(&mut diff_options))
			.expect("Can't `git diff`");
		let mut diff_txt = String::new();
		let _ = writeln!(diff_txt, "# Changes");
		let mut index = self.repo.index().unwrap();
		diff.print(git2::DiffFormat::Patch, |delta, hunk, line| {
			let Some(path) = delta.new_file().path().or(delta.old_file().path()) else {
				return true;
			};
			if self.silence_paths.iter().any(|p| p.matches_path(path)) {
				return true;
			}
			if self.noisy_paths.iter().any(|p| p.matches_path(path)) {
				return true;
			}
			match line.origin() {
				'+' | '-' | ' ' => {
					let _ = write!(diff_txt, "{}", line.origin());
				}
				_ => {}
			}
			let _ = write!(diff_txt, "{}", str::from_utf8(line.content()).unwrap());
			true
		})
		.unwrap();
		index.add_all(["."], IndexAddOption::DEFAULT, None).unwrap();
		for delta in diff.deltas() {
			let Some(path) = delta.new_file().path().or(delta.old_file().path()) else {
				continue;
			};
			if self.noisy_paths.iter().any(|p| p.matches_path(path)) {
				let _ = writeln!(diff_txt, "({path:#?} changed, but you don't know how.)");
			} else {
				match delta.status() {
					Delta::Untracked | Delta::Added => {
						let _ = writeln!(
							diff_txt,
							"{path:#?} created, its content follows:\n<content>\n{}\n</content>",
							tokio::fs::read_to_string(path).await.unwrap_or("<binary file>".into())
						);
					}
					Delta::Deleted => {
						let _ = writeln!(diff_txt, "{path:#?} deleted.");
					}
					Delta::Renamed => {
						let _ = writeln!(
							diff_txt,
							"{:#?} renamed to {:#?}.",
							delta.old_file().path().unwrap(),
							delta.new_file().path().unwrap()
						);
					}
					Delta::Copied => {
						let _ = writeln!(
							diff_txt,
							"{:#?} copied to {:#?}.",
							delta.old_file().path().unwrap(),
							delta.new_file().path().unwrap()
						);
					}
					_ => {}
				}
			}
		}
		index.write().unwrap();
		debug!("diff:\n{diff_txt}");
		self.state = State::Generate { diff_txt };
	}

	async fn generate(&mut self, diff_txt: String) {
		info!("Generate");
		let mut index = self.repo.index().unwrap();
		let mut config = OpenAIConfig::new().with_api_base(&self.config.endpoint);
		if let Some(token) = &self.config.token {
			config = config.with_api_key(token);
		}
		let client = async_openai::Client::build(reqwest::Client::new(), config);
		let chat = Chat::new(&client);

		let mut prompt = if let Some(prompt_file) = &self.config.prompt_file {
			tokio::fs::read_to_string(prompt_file)
				.await
				.expect("Can't read prompt.")
		} else {
			include_str!("default_prompt.md").to_string()
		};

		let request = chat.create_stream(CreateChatCompletionRequest {
			messages: vec![
				ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
					content: ChatCompletionRequestSystemMessageContent::Text(prompt),
					name: None,
				}),
				ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
					content:
						async_openai::types::chat::ChatCompletionRequestUserMessageContent::Text(
							diff_txt,
						),
					name: None,
				}),
			],
			model: self.config.model.clone(),
			verbosity: Some(Verbosity::Low),
			reasoning_effort: Some(if self.config.think {
				ReasoningEffort::Low
			} else {
				ReasoningEffort::None
			}),
			stream: Some(true),
			..CreateChatCompletionRequest::default()
		});

		let timeout = tokio::time::sleep(Duration::from_secs_f32(self.config.wait_timeout));
		let Some(response) = (select! {
			response = request => Some(response),
			_ = timeout => None,
		}) else {
			error!("Wait timeout.");
			self.state = State::Commit {
				commit_msg: "(LLM took too long to start writing.)".to_string(),
			};
			return;
		};
		let mut response = match response {
			Ok(r) => r,
			Err(e) => {
				error!("OpenAI error: {e}");
				self.state = State::Commit {
					commit_msg: "(LLM API failure.)".to_string(),
				};
				return;
			}
		};

		let timeout = Instant::now() + Duration::from_secs_f32(self.config.write_timeout);
		let mut commit_msg = String::new();
		print!("LLM message: ");
		loop {
			let step = response.next();
			let timeout = tokio::time::sleep_until(timeout);
			let Some(step) = (select! {
				step = step => Some(step),
				_ = timeout => None,
			}) else {
				println!();
				error!("Write timeout.");
				self.state = State::Commit {
					commit_msg: "(LLM took too long to finish writing.)".to_string(),
				};
				return;
			};
			let Some(step) = step else { break };
			let step = match step {
				Ok(r) => r,
				Err(e) => {
					println!();
					error!("OpenAI error: {e}");
					self.state = State::Commit {
						commit_msg: "(LLM API failure.)".to_string(),
					};
					return;
				}
			};
			let Some(choice) = step.choices.get(0) else {
				println!();
				error!("Malformed response.");
				let _ = writeln!(commit_msg, "(Malformed LLM response.)");
				break;
			};
			let delta = &choice.delta;
			if let Some(refusal) = &delta.refusal {
				println!();
				error!("LLM refusal: {refusal}");
				let _ = writeln!(commit_msg, "(LLM refusal.)");
				break;
			}
			let Some(content) = &delta.content else {
				continue;
			};
			print!("{content}");
			let _ = write!(commit_msg, "{content}");
		}
		println!();

		if let Some((_, output)) = commit_msg.split_once("</think>") {
			info!("Trimming thoughts from output.");
			commit_msg = output.trim().to_string();
		} else {
			commit_msg = commit_msg.trim().to_string();
		}

		self.state = State::Commit { commit_msg }
	}

	async fn commit(&mut self, commit_msg: String) {
		let mut index = self.repo.index().unwrap();
		let tree = index.write_tree().unwrap();
		let tree = self.repo.find_tree(tree).unwrap();
		let sig = self.repo.signature().unwrap();
		let message = format!("{}{commit_msg}", self.config.prefix);
		let head = self.repo.find_reference("HEAD").unwrap();
		let head = head.peel_to_commit().unwrap();
		self.repo
			.commit(Some("HEAD"), &sig, &sig, &message, &tree, &[&head])
			.unwrap();
		self.state = State::Idle;
	}
}
