//! `mycelium skills` — the agent guide the binary serves (docs/plans/identity-and-lifecycle.md, N5).
//!
//! A *skill* here is a guide a coding agent reads, not a prompt skill a node serves (`skill.invoke`). Each one
//! is a Markdown file under `docs/reference/agent-skills/`, embedded at compile time, so an installed binary carries
//! the guide that describes it and prints it without a checkout. [`Skill::text`] replaces
//! [`crate::init::TAG_PLACEHOLDER`] with the binary's own release tag, as `init` does for a scaffold.
//!
//! The guide is held to the code by this module's tests, not by a reader: its walk is the `one-hour` workflow's
//! commands, its subcommand table is the binary's dispatch and usage, its schema ids are `cli-json.md`'s.

use crate::init::{tag, TAG_PLACEHOLDER};

/// The schema id of `mycelium skills … --format json` (`docs/reference/cli-json.md`).
pub const SCHEMA: &str = "mycelium.skills/1";

/// One guide: its name, a one-line description, and the embedded Markdown before the tag is filled in.
pub struct Skill {
    pub name: &'static str,
    pub about: &'static str,
    source: &'static str,
}

impl Skill {
    /// The guide as served: the embedded text with the release tag filled in.
    pub fn text(&self) -> String {
        self.source.replace(TAG_PLACEHOLDER, &tag())
    }
}

/// Every guide this binary serves. A file under `docs/reference/agent-skills/` that is not listed here fails
/// `every_guide_on_disk_is_embedded`.
pub const SKILLS: &[Skill] = &[Skill {
    name: "fleet",
    about: "From the binary to a governed fleet in six steps, every subcommand, and how to read what they print",
    source: include_str!("../docs/reference/agent-skills/fleet.md"),
}];

/// The guide named `name`.
pub fn get(name: &str) -> Option<&'static Skill> {
    SKILLS.iter().find(|s| s.name == name)
}

/// `mycelium skills list --format json`.
pub fn list_json() -> serde_json::Value {
    let skills: Vec<_> = SKILLS.iter().map(|s| serde_json::json!({ "name": s.name, "about": s.about })).collect();
    serde_json::json!({ "schema": SCHEMA, "command": "list", "skills": skills })
}

/// `mycelium skills get <name> --format json`.
pub fn get_json(skill: &Skill) -> serde_json::Value {
    serde_json::json!({ "schema": SCHEMA, "command": "get", "name": skill.name, "about": skill.about, "text": skill.text() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    const FLEET: &str = include_str!("../docs/reference/agent-skills/fleet.md");
    const ONE_HOUR: &str = include_str!("../.github/workflows/one-hour.yml");
    const MAIN: &str = include_str!("main.rs");
    const CLI_JSON: &str = include_str!("../docs/reference/cli-json.md");
    const CONFIGURATION: &str = include_str!("../docs/reference/configuration.md");
    const ROOT_AGENTS: &str = include_str!("../AGENTS.md");
    const CLAUDE: &str = include_str!("../CLAUDE.md");

    /// The lines of every fenced block of `lang` in a Markdown text, in order.
    fn fenced(md: &str, lang: &str) -> Vec<String> {
        let (mut out, mut inside) = (Vec::new(), false);
        for line in md.lines() {
            if let Some(rest) = line.strip_prefix("```") {
                inside = !inside && rest.trim() == lang;
            } else if inside && !line.trim().is_empty() {
                out.push(line.trim().to_string());
            }
        }
        out
    }

    /// What the `one-hour` workflow runs: each invocation of the binary, of the SDK's install and of the scaffold's
    /// program — at the start of a line or inside `$(…)` — without its redirections, and with the job's own
    /// variables written as the guide writes them.
    fn workflow_commands(yml: &str) -> Vec<String> {
        const STARTS: [&str; 3] = ["mycelium ", "pip install ", "MYCELIUM_BIN=mycelium "];
        let mut out = Vec::new();
        for line in yml.lines() {
            let line = line.trim();
            let cmd = match line.split_once("$(") {
                Some((_, inner)) if STARTS.iter().any(|s| inner.starts_with(s)) => inner,
                _ if STARTS.iter().any(|s| line.starts_with(s)) => line,
                _ => continue,
            };
            let end = [" | ", " 2> ", " || ", ")"].iter().filter_map(|stop| cmd.find(stop)).min().unwrap_or(cmd.len());
            out.push(
                cmd[..end]
                    .trim()
                    .replace("\"$HOME/governed-call-py\"", "governed-call-py")
                    .replace("$REPO", "RichardTathata/mycelium")
                    .replace("$TAG", TAG_PLACEHOLDER),
            );
        }
        out
    }

    /// Every `mycelium.<name>/N` id a text names.
    fn schema_ids(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for (at, _) in text.match_indices("mycelium.") {
            let id: String = text[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-')).collect();
            let version = id.rsplit('/').next().unwrap_or("");
            if id.contains('/') && !version.is_empty() && version.chars().all(|c| c.is_ascii_digit()) {
                out.insert(id);
            }
        }
        out
    }

    /// The first backticked word of each row of the guide's subcommand table.
    fn guide_subcommands(md: &str) -> BTreeSet<String> {
        let section = md.split("## The subcommands").nth(1).expect("the guide has its subcommand table");
        let section = section.split("\n## ").next().unwrap_or(section);
        section
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .map(str::to_string)
            .collect()
    }

    /// The subcommands `main` dispatches: every `Some("<word>")` between `fn main` and the end of its dispatch, an
    /// option (`-V`) aside.
    fn dispatched_subcommands(main_rs: &str) -> BTreeSet<String> {
        let body = main_rs.split("fn main()").nth(1).expect("main.rs has fn main");
        let body = body.split("let config = parse_args()").next().expect("main parses the node's arguments after its dispatch");
        let mut out = BTreeSet::new();
        for (at, _) in body.match_indices("Some(\"") {
            let rest = &body[at + 6..];
            let end = rest.find(')').unwrap_or(rest.len());
            for word in rest[..end].split('|') {
                let word = word.trim().trim_matches('"');
                if !word.starts_with('-') {
                    out.insert(word.to_string());
                }
            }
        }
        out
    }

    /// The subcommands `--help` lists: the word after `mycelium` on each usage line that names one.
    fn usage_subcommands(main_rs: &str) -> BTreeSet<String> {
        let usage = main_rs.split("fn print_usage()").nth(1).expect("main.rs has print_usage");
        let usage = usage.split("Options:").next().expect("the usage lists its options after its subcommands");
        usage
            .match_indices("mycelium ")
            .map(|(at, _)| usage[at + 9..].split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).next().unwrap_or("").to_string())
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// **The row's exit: the guide's commands are the `one-hour` job's.** Every command in the guide's walk is one
    /// the workflow runs, in the workflow's order, and the workflow runs none the guide leaves out — so the walk an
    /// agent is handed is the one a release is measured on, and a step added to either fails here until the other
    /// has it.
    #[test]
    fn the_guides_commands_are_the_one_hour_jobs() {
        let guide = fenced(FLEET, "sh");
        let job = workflow_commands(ONE_HOUR);
        assert_eq!(guide, job, "the guide's ```sh lines, then the one-hour workflow's commands");
        assert_eq!(job.len(), 7, "the extraction found the workflow's commands: {job:?}");
        assert!(job.contains(&"mycelium test --faults grant units --config grant.toml".to_string()), "{job:?}");
    }

    /// The extraction sees a command wherever the workflow could put one, and nothing that only names the binary:
    /// so a new step cannot be run by the job and missed by the comparison above.
    #[test]
    fn the_workflow_extraction_sees_each_form_of_a_command() {
        let yml = "  got=$(mycelium --version)\n  mycelium init \"$HOME/governed-call-py\" --template x\n  \
                   mycelium test --faults grant units --config grant.toml 2> grant.err | tee grant.log || true\n  \
                   MYCELIUM_BIN=mycelium python3 main.py | tee run.log\n  pip install -q \"https://github.com/$REPO/archive/refs/tags/$TAG.tar.gz\"\n  \
                   grep -q 'mycelium explain --closure' recovery.log\n  install -m 0755 a/mycelium /usr/local/bin/mycelium\n  \
                   got=$(pip show mycelium-py | sed -n 1p)\n  # mycelium doctor in a comment\n";
        assert_eq!(
            workflow_commands(yml),
            [
                "mycelium --version",
                "mycelium init governed-call-py --template x",
                "mycelium test --faults grant units --config grant.toml",
                "MYCELIUM_BIN=mycelium python3 main.py",
                "pip install -q \"https://github.com/RichardTathata/mycelium/archive/refs/tags/{{MYCELIUM_TAG}}.tar.gz\"",
            ]
        );
    }

    /// What the guide tells an agent to expect is what the workflow greps for, and the configuration it tells an
    /// agent to write is what the workflow writes, line for line, in tables the configuration reference has a row for.
    #[test]
    fn the_guides_expectations_and_configuration_are_the_one_hour_jobs() {
        let mut expected = 0;
        for line in FLEET.lines().filter(|l| l.starts_with("Expect: ")) {
            for quoted in line.split('`').skip(1).step_by(2) {
                let grepped = ONE_HOUR.contains(&format!("grep -q '{quoted}'")) || ONE_HOUR.contains(&format!("grep -q '^{quoted}'"));
                assert!(grepped, "the workflow greps for no line `{quoted}`");
                expected += 1;
            }
        }
        assert_eq!(expected, 6, "every step from 3 on says what to expect");
        let toml = fenced(FLEET, "toml");
        assert_eq!(toml.len(), 6, "{toml:?}");
        for line in toml {
            assert!(ONE_HOUR.contains(&line), "the workflow writes no `{line}`");
            if let Some(table) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                assert!(CONFIGURATION.contains(&format!("\n| {table} |")), "docs/reference/configuration.md has no row for `{table}`");
            }
        }
    }

    /// **Every subcommand the binary accepts is in the guide, and the guide names no other**: the guide's table, the
    /// arms `main` dispatches and the usage `--help` prints are one set.
    #[test]
    fn the_guides_subcommands_are_the_binarys() {
        let (guide, dispatched, usage) = (guide_subcommands(FLEET), dispatched_subcommands(MAIN), usage_subcommands(MAIN));
        assert!(dispatched.contains("wire-check") && dispatched.contains("skills") && dispatched.len() >= 9, "{dispatched:?}");
        assert_eq!(guide, dispatched, "the guide's table, then main's dispatch");
        assert_eq!(usage, dispatched, "the usage, then main's dispatch");
    }

    /// The schema ids the guide names are exactly the ones `docs/reference/cli-json.md` lists.
    #[test]
    fn the_guides_schema_ids_are_the_references() {
        let (guide, reference) = (schema_ids(FLEET), schema_ids(CLI_JSON));
        assert!(reference.contains(SCHEMA) && reference.contains("mycelium.design/declaration/1"), "{reference:?}");
        assert_eq!(guide, reference, "the guide's ids, then cli-json.md's");
    }

    /// **The size, gated**: the root `AGENTS.md` is at most 2 KB (the row's bound), and a guide at most 6 KB, since
    /// an agent reads each whole.
    #[test]
    fn the_sizes_are_bounded() {
        assert!(ROOT_AGENTS.len() <= 2048, "AGENTS.md is {} bytes; the bound is 2048", ROOT_AGENTS.len());
        for s in SKILLS {
            assert!(s.text().len() <= 6144, "the `{}` guide is {} bytes; the bound is 6144", s.name, s.text().len());
            assert!(!s.about.is_empty() && s.about.len() <= 120, "{}", s.about);
        }
    }

    /// The root `AGENTS.md` sends its reader to the guide the binary serves and to this repository's own rules, and
    /// **`CLAUDE.md` names both**: Claude Code reads a root `AGENTS.md` only where there is no `CLAUDE.md`, so that
    /// line is the only way a Claude Code session here learns of either.
    #[test]
    fn the_root_agents_file_and_the_claude_line_name_the_guide() {
        assert!(ROOT_AGENTS.contains("mycelium skills get fleet"), "AGENTS.md names the command");
        assert!(ROOT_AGENTS.contains("CLAUDE.md") && ROOT_AGENTS.contains("docs/wiki/AGENTS.md"), "AGENTS.md names the rules and the wiki's schema");
        let line = CLAUDE.lines().find(|l| l.contains("mycelium skills get fleet")).expect("CLAUDE.md names the command");
        assert!(line.contains("AGENTS.md"), "the same line names the root AGENTS.md: {line}");
    }

    /// **Nothing of the machine that built it**: no absolute or home path, no credential, no key block, in what the
    /// binary serves or in `AGENTS.md`; the only URL is the SDK's archive at the binary's own tag; and no relative
    /// link, which would lead nowhere from a terminal.
    #[test]
    fn nothing_served_names_a_machine_or_a_secret() {
        let served: Vec<String> = SKILLS.iter().map(Skill::text).chain([ROOT_AGENTS.to_string(), list_json().to_string()]).collect();
        for text in &served {
            for bad in ["/Users/", "/home/", "/Volumes/", "/tmp/", "/private/", "/root/", "$HOME", "~/", "C:\\", "-----BEGIN", "Bearer ", "token =", "api_key", "password", env!("CARGO_MANIFEST_DIR")] {
                assert!(!text.contains(bad), "`{bad}` in served text");
            }
            assert!(!text.contains(TAG_PLACEHOLDER), "the tag is filled in");
        }
        let fleet = get("fleet").unwrap().text();
        assert!(!fleet.contains("]("), "a link in the guide");
        let urls: Vec<&str> = fleet.match_indices("https://").map(|(at, _)| fleet[at..].split('"').next().unwrap()).collect();
        assert_eq!(urls, [format!("https://github.com/RichardTathata/mycelium/archive/refs/tags/{}.tar.gz#subdirectory=mycelium-py", tag())]);
        assert!(!fleet.contains("http://"));
    }

    /// The directory and the embedded list hold the same guides with the same bytes.
    #[test]
    fn every_guide_on_disk_is_embedded() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/agent-skills");
        let mut on_disk = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).expect("the guides' directory") {
            let path = entry.expect("entry").path();
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            assert_eq!(path.extension().and_then(|e| e.to_str()), Some("md"), "{}", path.display());
            on_disk.insert((name, std::fs::read_to_string(&path).expect("a guide is UTF-8 text")));
        }
        let embedded: BTreeSet<_> = SKILLS.iter().map(|s| (s.name.to_string(), s.source.to_string())).collect();
        assert_eq!(on_disk, embedded);
        assert!(get("fleet").is_some() && get("nope").is_none());
    }

    /// The JSON forms: the list names every guide, and `get` carries the served text whole.
    #[test]
    fn the_json_forms_carry_the_list_and_the_text() {
        let list = list_json();
        assert_eq!(list["schema"], SCHEMA);
        assert_eq!(list["skills"].as_array().unwrap().len(), SKILLS.len());
        let got = get_json(get("fleet").unwrap());
        assert_eq!(got["text"].as_str().unwrap(), get("fleet").unwrap().text());
        assert!(got["text"].as_str().unwrap().contains(&format!("/tags/{}.tar.gz", tag())));
    }
}
