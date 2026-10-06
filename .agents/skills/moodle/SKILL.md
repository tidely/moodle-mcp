---
name: moodle
description: Use `moodle` to answer questions about the user's Moodle courses — deadlines, what to work on next, assignment/project descriptions, course materials and files. Mirrors courses to local folders that you then read and grep.
---

# moodle

Read-only access to Moodle. Nothing is ever submitted or changed. Mirror a course locally, then use your own file tools on the mirror.

## Output contract

- stdout is JSON (compact when piped). Filter with `jq`.
- Errors are JSON on stderr: `{"error":{"code","message","hint"}}`. Follow the `hint`.
- Exit codes: `0` ok, `1` error, `2` partial (some downloads failed; see `failed`).

## Login

If you get `not_logged_in`, `invalidtoken` or `interactive_required`, ask the user to run this in their own terminal (it needs a browser):

```sh
moodle login --url https://their.moodle.site/
```

Don't ask the user to paste their token into the chat.

## Commands

| Command                                 | Use                                                                    |
| --------------------------------------- | ---------------------------------------------------------------------- |
| `moodle todo [--days 14] [--course ID]` | Deadlines and actions across all courses. Start here for "what's due". |
| `moodle courses [--all]`                | Course ids (current courses unless `--all`).                           |
| `moodle clone <course_id> [dir]`        | Mirror a course into `./<shortname> (<id>)/`.                          |
| `moodle pull`                           | Update the mirror you're in (or `-C <dir>`).                           |
| `moodle status`                         | Offline: last sync, skipped/failed files.                              |
| `moodle fetch <path\|cmid>... \| --all` | Download files skipped by the size limit (100 MB default).             |
| `moodle whoami`                         | Check login and site.                                                  |

Mirror commands work from any subdirectory of a mirror, like git. If a mirror already exists, `pull` it instead of cloning again. You are encouraged to `clone` courses when you need additional insight.

## Mirror layout

```
<shortname> (<id>)/
├── index.md                  # course summary, sections → activities, dates
├── _embedded/                # images from the course page
└── <activity name> (<cmid>)/
    ├── index.md              # description, dates, submission status, grade, file list
    ├── <files>               # attachments, resources, folder contents
    ├── submission/, feedback/
    └── _embedded/            # images/files linked from the description
```

## Workflow

1. Deadlines: `moodle todo`. Run inside a mirror and items include `index` paths to the local activity.
2. Course content: find the id with `courses`, then `clone` it (or `pull` an existing mirror), then read `index.md`.
3. Search: `grep -ri '<term>' .` across the mirror. For PDFs use your own tools (e.g. `pdftotext`).
4. Before trusting dates, submission status or grades, run `pull`. Check `synced_at` with `status`.
5. Files marked "not downloaded" in an index: `moodle fetch '<path>'`.

Quote paths: directory names contain spaces and parentheses.
