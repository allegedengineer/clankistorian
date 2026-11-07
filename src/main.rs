use std::{env::current_dir, path::PathBuf};

use clap::Parser;
use clap_verbosity_flag::Verbosity;
use figment::{Figment, providers::Format};
use tracing::{error, info, instrument::WithSubscriber};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;

pub mod cmd;
pub mod config;
pub mod worker;

#[derive(clap::Parser)]
pub struct Args {
	#[command(subcommand)]
	pub verb: Verb,
	#[command(flatten)]
	pub config: config::Config,
	#[command(flatten)]
	verbosity: Verbosity,
}

#[derive(clap::Subcommand)]
pub enum Verb {
	/// Open $EDITOR to clankistorian.toml.
	Config {
		/// Open the `clankistorian.toml` at the root of the nearest ancestor git repo.
		#[arg(short, long)]
		local: bool,
	},
	/// Start in the working directory.
	/// (This will first stop any existing session.)
	Start,
	/// Determine if a clankistorian is running here and report its status.
	Status,
	/// Stop an existing session.
	Stop,
	/// Prevent an existing session from starting again, but allow it to finish what it's up to.
	Pause,
	/// Allow an existing session to make commits again.
	Resume,
	/// Force an existing session to make a commit.
	Force,
}

#[tokio::main]
async fn main() {
	let args = Args::parse();

	init_tracing(&args);

	let editor = std::env::var("EDITOR")
		.ok()
		.filter(|v| !v.is_empty())
		.unwrap_or_else(|| "vi".to_string());

	let global_config = dirs::config_dir()
		.expect("Can't find configuration dir")
		.join("clankistorian.toml");
	if let Verb::Config { local: false } = &args.verb {
		edit_file(&editor, global_config);
	}

	let root = find_root().expect("Can't find a repo to use as our root.");

	let local_config = root.join("clankistorian.toml");
	if let Verb::Config { local: true } = &args.verb {
		edit_file(&editor, global_config);
	}

	info!("clankistorian running in {root:#?}");

	let config: Config = {
		let figment = Figment::new().merge(figment::providers::Serialized::defaults(&args.config));
		let merge = if global_config.exists() {
			figment.merge(figment::providers::Toml::file_exact(&global_config))
		} else {
			figment
		};
		merge
			.merge(figment::providers::Toml::file(&local_config))
			.extract()
			.expect("Failed to load config")
	};

	match args.verb {
		Verb::Config { .. } => unreachable!(),
		Verb::Start => {
			let worker = worker::Worker::new(config, root).await;
			worker.run().await;
		}
		Verb::Status => todo!(),
		Verb::Stop => todo!(),
		Verb::Pause => todo!(),
		Verb::Resume => todo!(),
		Verb::Force => todo!(),
	}
}

fn init_tracing(args: &Args) {
	let registry = tracing_subscriber::registry()
		.with(tracing_subscriber::fmt::layer().with_filter(args.verbosity.tracing_level_filter()));
	if let Ok(journald) = tracing_journald::layer() {
		registry.with(journald).init();
	} else {
		registry.init();
		error!("Can't reach journald. Proceeding with stderr.")
	};
}

fn edit_file(editor: &str, conf_file: PathBuf) -> ! {
	let mut editor = editor.split_ascii_whitespace();
	std::process::Command::new(editor.next().unwrap())
		.args(editor)
		.arg(conf_file)
		.spawn()
		.expect("Failed to spawn editor")
		.wait()
		.unwrap();
	std::process::exit(0);
}

/// Returns the nearest ancestor git repository.
fn find_root() -> Option<PathBuf> {
	let mut path = current_dir().ok()?;
	while !path.join(".git").exists() && !path.pop() {}
	path.join(".git").exists().then_some(path)
}
