# Clankistorian

(clanker historian.)

A tool that watches your git repository and generates "so-so" commit messages while you work using a local LLM.

## Advice on using Clankistorian

Please don't push its commits publicly. They won't be any good. Clankistorian was built to solve a common problem: Everyone knows they're supposed to make atomic commits as they work, but no one wants to context-switch. You should let it run while you work, then use an interactive rebase to break its commits up and squash them down into more meaningful units once you're done.

This repository has unfiltered Clankistorian output in its history as a demonstration of the kind of commits it can produce, not as an example of correct usage. Do as I say, not as I do.

## Quick start

* Install Ollama.
* Install clankistorian: `cargo install --git https://github.com/allegedengineer/clankistorian`
* Write a `clankistorian.toml` to configure the repository where you're using it.
  * This isn't terribly well-documented, see [config.rs](src/config.rs) for details.
  * You should at least configure `silent_files` and `noisy_files` so that your `Cargo.lock` doesn't overflow the context window.
