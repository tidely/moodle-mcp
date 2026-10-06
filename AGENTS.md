# AGENTS.md

## Objective

Rust MCP server that lets students query Moodle: courses, project descriptions, assignments, and files.

## Stack

- Rust (edition 2024), official MCP SDK: [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk).
- Read-only. Never submit, post, or modify anything on Moodle.
- Cargo workspace:
  - `crates/moodle-api`: Moodle web service protocol types (request/response pairs via `WsFunction`).
  - `crates/moodle-sync`: mirrors a course into a given directory (files + markdown indexes). Knows nothing about MCP.
  - `crates/moodle-mcp`: the MCP server binary. Thin layer over `moodle-sync`.
  - `crates/moodle-mock`: mock Moodle HTTP server for tests (dev-dependency only).
  - `crates/moodle-cli`: agent-first CLI (`login`, `courses`, `todo`, `clone`, `pull`, `status`, `fetch`). Thin layer over `moodle-sync`.
- Define all dependencies (versions, features) in the root `Cargo.toml` under `[workspace.dependencies]`. Crates reference them with `dep.workspace = true`.

## Moodle access

- Reference implementation: `../Moodle-DL` (Python). Its auth works with our SSO; mirror it.
- SSO login: open `{base}admin/tool/mobile/launch.php?service=moodle_mobile_app&passport={random}&urlscheme=moodledl` in a browser, then capture the redirect `moodledl://token=<base64>`.
  Decode the base64, split on `:::`. `[1]` is the token, `[2]` (optional) is the private token. See `moodle_dl/moodle/moodle_service.py::extract_token`.
- API: `POST {base}webservice/rest/server.php?moodlewsrestformat=json&wsfunction=<fn>` with form fields `wstoken`, `wsfunction`, and params. See `moodle_dl/moodle/request_helper.py`.
- Use the MoodleMobile User-Agent from `request_helper.py`. Some instances reject other UAs.
- File URLs require `?token=<wstoken>` appended.
- Per-module logic (assign, folder, page, book, etc.) lives in `moodle_dl/moodle/mods/`.

## Course mirror (`moodle-sync`)

- Agents work on a local mirror with their own tools (grep, read, etc.), not through a browsing API.
- Course dir layout: `index.md` (course summary, sections → activities, dates), and per activity `{activity name} ({cmid})/index.md` (description, dates, submission status, file list) next to its files at `{filepath}/{filename}`.
  Files only referenced from HTML (pasted images, inline links to Moodle files) go to `_embedded/` in the course or activity dir.
- Index files are markdown (HTML converted). Never write Moodle file URLs into them; link local paths instead.
- Skip files above a size limit (default 100 MB) and list them as not downloaded.
- Sanitize names (no `..` or `/` escapes). Skip unchanged files by `filesize` + `timemodified`.

## CLI (`moodle-cli`)

- Agents are the primary user. Output is JSON on stdout (pretty on a TTY, compact otherwise). Never print tables.
- Errors are JSON on stderr: `{"error":{"code","message","hint"}}`. Every error an agent can act on gets a `hint` with the next command.
- Exit codes: 0 ok, 1 error, 2 partial (some downloads failed).
- Git-like mirrors: `clone` creates a local dir with `.moodle/mirror.json` (site, course id, size limit); mirror commands find it by walking up from the cwd (or `-C`).
- `.moodle/manifest.json` (written by `moodle-sync`) records every file and its state (`present`/`skipped`/`failed`) and `synced_at`. Pruning only deletes files listed there.
- Credentials: `MOODLE_URL`/`MOODLE_TOKEN` env (or `.env`), else `~/.config/moodle-cli/credentials.json` (0600) from `login`.
- Non-interactive by default; anything needing a human (browser SSO) fails with a hint when stdin isn't a TTY.

## MCP tools

- `list_courses`, `sync_course(course_id)`, `download(cmid, files?)` (for files skipped by the size limit).
- Return compact markdown with absolute paths. Never return file URLs.
- Global cache root: `~/.moodle-mcp/` (override with `MOODLE_MCP_DIR`). Agents read documents from there.
  Course dir: `files/{host}/{course shortname} ({id})/`.

## Rules

- Never log or commit tokens. Redact `token=` in output.
- Keep it simple. Don't add dependencies without a reason.
