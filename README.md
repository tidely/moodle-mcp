# moodle-mcp

Let AI agents read your Moodle courses: deadlines, project descriptions, assignments and course files.

The main tool is `moodle`. It mirrors a course into a local folder, like `git clone`: course materials plus markdown `index.md` files with descriptions, dates and submission status. Agents then read and search the mirror with their normal file tools. Its output is JSON, built for agents first.

Everything is **read-only**. It never submits or changes anything on Moodle.

## Install

You need [Rust](https://rustup.rs/).

```sh
cargo install --git https://github.com/tidely/moodle-mcp cli
```

The Cargo package is named `cli`; it installs the `moodle` command.

Log in once. This opens your Moodle's SSO page and stores the token in `~/.config/moodle-cli/credentials.json`, readable only by you:

```sh
moodle login --url https://your.moodle.site/
```

After logging in, the browser tries to open a `moodledl://token=...` link and fails. Copy that link (e.g. from the dev tools network tab) and paste it into the terminal.

Alternatively, set `MOODLE_URL` and `MOODLE_TOKEN` in the environment or a `.env` file (see `example.env`).

## Usage

```sh
moodle todo                 # what's due next, across all courses
moodle courses              # your current courses and their ids
moodle clone 4214           # mirror a course into "./<shortname> (4214)/"
cd "<shortname> (4214)"
moodle pull                 # update the mirror
moodle status               # skipped or failed files
moodle fetch --all          # download files skipped by the size limit
```

Run `moodle --help` for all options.

## Use it with an agent

Give your agent the skill in [`.agents/skills/moodle-cli/SKILL.md`](.agents/skills/moodle-cli/SKILL.md). It explains the commands, the mirror layout and the workflow.

- Agents that load project skills (e.g. Zed) pick it up automatically when this repository is open.
- To use it everywhere, copy it to your global skills folder:

  ```sh
  mkdir -p ~/.agents/skills && cp -r .agents/skills/moodle ~/.agents/skills/
  ```

## Repository layout

| Crate         | Purpose                                                           |
| ------------- | ----------------------------------------------------------------- |
| `crates/api`  | Moodle web service request/response types and a small HTTP client |
| `crates/sync` | Mirrors a course into a directory (files + markdown indexes)      |
| `crates/cli`  | The agent-first CLI                                               |
| `crates/mcp`  | An MCP server over the same library, for clients without a shell  |
| `crates/mock` | Mock Moodle server for tests                                      |

The Moodle login flow and API usage follow [Moodle-DL](https://github.com/C0D3D3V/Moodle-DL).

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```
