use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub enum Command {
	/// Finish what you're doing, then exit.
	Stop,
	/// Ignore new changes.
	Pause,
	/// Stop ignoring new changes.
	Resume,
	/// Try to commit now.
	Force,
}
