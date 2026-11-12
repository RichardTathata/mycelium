//! `mycelium skills` — the agent guide the binary serves (docs/plans/identity-and-lifecycle.md, N5).
//!
//! A *skill* here is a guide a coding agent reads, not a prompt skill a node serves (`skill.invoke`). Each one
//! is a Markdown file under `docs/reference/agent-skills/`, embedded at compile time, so an installed binary carries
//! the guide that describes it and prints it without a checkout. [`Skill::text`] replaces
//! [`crate::init::TAG_PLACEHOLDER`] with the binary's own release tag, as `init` does for a scaffold.
//!
//! The guide is held to the code by this module's tests, not by a reader: its walk is the `one-hour` workflow's
//! commands, its subcommand table is the binary's dispatch and usage, its schema ids are `cli-json.md`'s. What the
//! tests do not hold is said where they are: a subcommand's sub-verbs and flags are not compared.

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
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    const FLEET: &str = include_str!("../docs/reference/agent-skills/fleet.md");
    const ONE_HOUR: &str = include_str!("../.github/workflows/one-hour.yml");
    const RELEASE: &str = include_str!("../.github/workflows/release.yml");
    const MAIN: &str = include_str!("main.rs");
    const CLI_JSON: &str = include_str!("../docs/reference/cli-json.md");
    const CONFIGURATION: &str = include_str!("../docs/reference/configuration.md");
    const ROOT_AGENTS: &str = include_str!("../AGENTS.md");
    const CLAUDE: &str = include_str!("../CLAUDE.md");
    const SCAFFOLD_NODE: &str = include_str!("../templates/governed-call-py/node.toml");

    /// Programs whose invocation is a command of the walk.
    const RUNNERS: [&str; 6] = ["mycelium", "pip", "pip3", "python", "python3", "uv"];
    /// Programs that may name the binary, the SDK or Python in their arguments without running any of them. A
    /// segment that mentions one of the three and starts with a program in neither list fails the extraction.
    const NOT_RUNNERS: [&str; 14] = ["echo", "grep", "curl", "tar", "install", "[", "set", "test", "cd", "printf", "cat", "exit", "true", "}"];
    /// What the workflow runs and the guide leaves out: two queries of the job's own environment.
    const WORKFLOW_ONLY: [&str; 2] = ["python3 --version", "pip show mycelium-py"];
    /// **The one stated difference**: what the guide runs and the workflow does not. The job's container has its own
    /// Python; an OS-managed one refuses a bare `pip install` (tutorial 7, *On another machine*).
    const GUIDE_ONLY: [&str; 1] = ["python3 -m venv .venv && . .venv/bin/activate"];
    /// Inline code in the guide that invokes a runner without being a step of the walk.
    const INLINE_ALLOWED: [&str; 1] = ["mycelium skills get fleet"];
    /// What the workflow greps for and the guide does not quote, by step. A grep added to the workflow fails
    /// `the_guides_expectations_and_configuration_are_the_one_hour_jobs` until it is quoted in the guide or listed here.
    const NOT_QUOTED: [(u32, &str); 10] = [
        (3, "alice declared `purchaser`"),
        (3, "not enforced by this node"),
        (3, "the route needs kv:write"),
        (4, "✓ a grant-required tool is default-deny"),
        (4, "✓ issuing is the declaration's rule"),
        (4, "✓ the grant holds across a restart"),
        (5, "✓ the restarted node finds its lost-acknowledgement record unknown in its own ledger, probes before any retry, and the destination holds one effect"),
        (5, "✓ the destination saw each effect once"),
        (5, "mycelium explain --closure"),
        // A negative check: the job fails if this line IS printed.
        (6, "gw.token_tables:"),
    ];
    /// Each subcommand's `--format json` schema ids, and the source file that holds the literal.
    const CLI_SCHEMAS: [(&str, &[(&str, &str)]); 9] = [
        ("init", &[]),
        ("declare", &[("mycelium.declare/1", include_str!("declare.rs"))]),
        ("doctor", &[("mycelium.doctor/1", include_str!("doctor.rs"))]),
        ("wire-check", &[("mycelium.design/declaration/1", include_str!("wire_check.rs"))]),
        ("rules", &[("mycelium.rules/1", include_str!("../mycelium-core/src/rule.rs"))]),
        ("test", &[("mycelium.test-faults/1", include_str!("faults.rs"))]),
        ("explain", &[("mycelium.explain/1", include_str!("../mycelium-core/src/decision.rs")), ("mycelium.closure/1", include_str!("closure.rs"))]),
        ("tls", &[("mycelium.tls/1", include_str!("main.rs"))]),
        ("skills", &[(SCHEMA, include_str!("agent_guide.rs"))]),
    ];
    /// Schema ids in those source files that are records a command reads, not documents a command prints.
    const NOT_CLI_DOCUMENTS: [&str; 3] = ["mycelium.decision/1", "mycelium.coverage/1", "mycelium.ae/evidence/1"];

    /// Every fenced block of a Markdown text: its tag and its non-empty lines.
    fn fences(md: &str) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        let mut inside = false;
        for line in md.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("```") {
                inside = !inside;
                if inside {
                    out.push((rest.trim().to_string(), Vec::new()));
                }
            } else if inside && !line.trim().is_empty() {
                out.last_mut().unwrap().1.push(line.trim().to_string());
            }
        }
        assert!(!inside, "an unclosed fence");
        out
    }

    /// The lines of every fenced block tagged `lang`, in order.
    fn fenced(md: &str, lang: &str) -> Vec<String> {
        fences(md).into_iter().filter(|(tag, _)| tag == lang).flat_map(|(_, lines)| lines).collect()
    }

    /// A workflow's shell lines by step: the number a step's name opens with (`"3 · …"`; 0 without one), and every
    /// line of its `run:`, in the block form (`run: |`) and the single-line form (`run: cmd`). Comment lines are
    /// left out.
    fn run_lines(yml: &str) -> Vec<(u32, String)> {
        let (mut out, mut step, mut block): (Vec<(u32, String)>, u32, Option<usize>) = (Vec::new(), 0, None);
        for line in yml.lines() {
            let indent = line.len() - line.trim_start().len();
            let trimmed = line.trim();
            if let Some(at) = block {
                if trimmed.is_empty() || indent > at {
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        out.push((step, trimmed.to_string()));
                    }
                    continue;
                }
                block = None;
            }
            let key = trimmed.strip_prefix("- ").unwrap_or(trimmed);
            if let Some(name) = key.strip_prefix("name:") {
                let name = name.trim().trim_matches('"');
                step = name.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            } else if let Some(rest) = key.strip_prefix("run:") {
                let rest = rest.trim();
                if rest.starts_with('|') || rest.starts_with('>') {
                    block = Some(indent);
                } else if !rest.is_empty() {
                    out.push((step, rest.to_string()));
                }
            } else if let Some(action) = key.strip_prefix("uses:") {
                // A local action's steps are in another file, which this extraction does not read.
                assert!(!action.trim().trim_matches(|c| c == '"' || c == '\'').starts_with('.'), "a local action, whose commands are not read: `{trimmed}`");
            } else if let Some((name, value)) = key.split_once(':') {
                // An `env:` entry that holds a runner: a later `"$NAME" …` would run it unseen.
                let variable = !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                assert!(!(variable && mentions_a_runner(value)), "an env entry that holds the binary, pip or Python: `{trimmed}`");
            }
        }
        out
    }

    /// Whether a text names the binary, pip or Python.
    fn mentions_a_runner(text: &str) -> bool {
        ["mycelium", "pip", "python"].iter().any(|m| text.contains(m))
    }

    /// A shell line as its simple commands: split at `&&`, `||`, `;` and `|` outside quotes, with the inside of
    /// every `$(…)` — at any depth, inside double quotes too — as commands of its own (the outer one keeps `$()` in
    /// its place). A backtick substitution is refused: the extraction does not read one.
    fn segments(line: &str, out: &mut Vec<String>) {
        let b: Vec<char> = line.chars().collect();
        let (mut cur, mut i, mut single, mut double) = (String::new(), 0, false, false);
        while i < b.len() {
            let (c, next) = (b[i], b.get(i + 1).copied());
            if single {
                single = c != '\'';
                cur.push(c);
                i += 1;
            } else if c == '\\' {
                cur.push(c);
                cur.extend(next);
                i += 2;
            } else if c == '$' && next == Some('(') {
                let (mut depth, mut j) = (1, i + 2);
                while j < b.len() && depth > 0 {
                    match b[j] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                assert_eq!(depth, 0, "an unclosed $( in: {line}");
                segments(&b[i + 2..j - 1].iter().collect::<String>(), out);
                cur.push_str("$()");
                i = j;
            } else if c == '`' {
                panic!("a backtick substitution, which the extraction does not read; write $(…): {line}");
            } else if c == '"' {
                double = !double;
                cur.push(c);
                i += 1;
            } else if !double && c == '\'' {
                single = true;
                cur.push(c);
                i += 1;
            } else if !double && (c == ';' || c == '|' || c == '&') {
                out.push(std::mem::take(&mut cur));
                i += if next == Some(c) { 2 } else { 1 };
            } else {
                cur.push(c);
                i += 1;
            }
        }
        out.push(cur);
        out.retain(|s| !s.trim().is_empty());
    }

    /// The command a segment runs, when its program is one of [`RUNNERS`]: without `sudo`, `env`, `time`, `!`, `if`
    /// and the like before it, with the program's directory dropped, its own variable assignments kept, and cut at
    /// a redirection. `Ok(None)` for an assignment that holds no runner, or a program in [`NOT_RUNNERS`]. **`Err`**
    /// for what could run a command unseen: a program held in a variable (`"$BIN" rules`), an assignment whose
    /// value names a runner (`BIN=/usr/local/bin/mycelium`), a local script (`./walk.sh`, `bash scripts/step.sh`),
    /// and any other program whose segment mentions the binary, pip or Python.
    fn classify(segment: &str) -> Result<Option<String>, String> {
        let mut words: Vec<&str> = segment.split_whitespace().collect();
        while matches!(words.first(), Some(&("sudo" | "env" | "time" | "!" | "if" | "then" | "elif" | "else" | "while" | "do" | "{" | "("))) {
            words.remove(0);
        }
        let assignment = |w: &str| w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let at = words.iter().position(|w| !assignment(w)).unwrap_or(words.len());
        let holds_a_runner = words[..at].iter().any(|w| mentions_a_runner(w.split_once('=').map_or("", |(_, v)| v)));
        let Some(word) = words.get(at).map(|w| w.trim_matches('"')) else {
            return if holds_a_runner { Err(format!("a variable that holds a runner, which a later line could run unseen: `{segment}`")) } else { Ok(None) };
        };
        let program = word.rsplit('/').next().unwrap_or("");
        if RUNNERS.contains(&program) {
            let mut out: Vec<&str> = words[..at].to_vec();
            out.push(program);
            out.extend(words[at + 1..].iter().take_while(|w| !w.starts_with('>') && !w.starts_with("2>") && !w.starts_with('<')));
            return Ok(Some(
                out.join(" ")
                    .replace("\"$HOME/governed-call-py\"", "governed-call-py")
                    .replace("$REPO", "RichardTathata/mycelium")
                    .replace("$TAG", TAG_PLACEHOLDER),
            ));
        }
        if holds_a_runner {
            return Err(format!("a variable that holds a runner, set for a program that is not one: `{segment}`"));
        }
        if word.starts_with('$') && word != "$()" {
            return Err(format!("a program held in a variable, which the extraction cannot name: `{segment}`"));
        }
        let shell = matches!(program, "bash" | "sh" | "zsh" | "source" | ".") && words[at + 1..].iter().any(|w| !w.starts_with('-'));
        if shell || word.starts_with("./") || word.starts_with("../") || program.ends_with(".sh") || program.ends_with(".py") {
            return Err(format!("a script, whose commands are not read: `{segment}`"));
        }
        if mentions_a_runner(segment) && !NOT_RUNNERS.contains(&program) {
            return Err(format!("unclassified: `{program}` in `{segment}` — a runner, or a program that only names one?"));
        }
        Ok(None)
    }

    /// [`classify`] for a workflow line, where an unclassified segment **fails** and is never skipped.
    fn command_of(segment: &str) -> Option<String> {
        classify(segment).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Every command the workflow runs, in order: [`run_lines`], [`segments`], [`command_of`].
    fn workflow_commands(yml: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (_, line) in run_lines(yml) {
            let mut segs = Vec::new();
            segments(&line, &mut segs);
            // A substitution's commands run before the command that holds it; within a line, keep the line's order.
            out.extend(segs.iter().filter_map(|s| command_of(s)));
        }
        out
    }

    /// A shell word without its quoting: `'a'"'"'b'` is `a'b`, `"a \`b\`"` is ``a `b` ``.
    fn unquote(word: &str) -> String {
        let (mut out, mut single, mut double) = (String::new(), false, false);
        let mut chars = word.chars();
        while let Some(c) = chars.next() {
            match c {
                '\'' if !double => single = !single,
                '"' if !single => double = !double,
                '\\' if double => out.extend(chars.next()),
                _ => out.push(c),
            }
        }
        out
    }

    /// What each numbered step of the workflow greps its output for, a leading `^` dropped: `grep` with a `q` among
    /// its short flags (`-q`, `-Eq`, `-qF`), then the pattern, then a file or nothing. **Not seen:** an output
    /// checked another way — `grep -c`, a `[ "$got" = … ]` comparison — and a grep outside the numbered steps.
    fn workflow_expectations(yml: &str) -> BTreeMap<u32, BTreeSet<String>> {
        let mut out: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
        for (step, line) in run_lines(yml) {
            let mut segs = Vec::new();
            segments(&line, &mut segs);
            for seg in segs {
                let seg = seg.trim().trim_start_matches("if ").trim_start_matches("! ");
                let Some(mut rest) = seg.strip_prefix("grep ").map(str::trim) else { continue };
                let mut quiet = false;
                while let Some((flag, after)) = rest.split_once(' ').filter(|(flag, _)| flag.starts_with('-')) {
                    quiet |= !flag.starts_with("--") && flag.contains('q');
                    rest = after.trim();
                }
                if quiet && step > 0 {
                    let pattern = if rest.ends_with('\'') || rest.ends_with('"') { rest } else { rest.rsplit_once(' ').map_or(rest, |(pattern, _file)| pattern) };
                    out.entry(step).or_default().insert(unquote(pattern).trim_start_matches('^').to_string());
                }
            }
        }
        out
    }

    /// The lines each step of the workflow appends to `node.toml` (`printf '…' … > x.toml`).
    fn workflow_configuration(yml: &str) -> BTreeMap<u32, Vec<String>> {
        let mut out = BTreeMap::new();
        for (step, line) in run_lines(yml) {
            if let Some((_, rest)) = line.split_once("printf '")
                && line.trim_end().ends_with(".toml")
            {
                let text = rest.split('\'').next().unwrap().replace("\\n", "\n");
                out.insert(step, text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect());
            }
        }
        out
    }

    /// The guide's walk by step: the text from `N. ` to the next step or the next heading.
    fn guide_steps(md: &str) -> BTreeMap<u32, String> {
        let walk = md.split("## The walk").nth(1).expect("the guide has its walk");
        let walk = walk.split("\n## ").next().unwrap();
        let mut out: BTreeMap<u32, String> = BTreeMap::new();
        let mut step = 0;
        for line in walk.lines() {
            if let Some(n) = line.split_once(". ").and_then(|(n, _)| n.parse::<u32>().ok()) {
                step = n;
            }
            let text = out.entry(step).or_default();
            text.push_str(line);
            text.push('\n');
        }
        out
    }

    /// Every inline code span of a Markdown text, fenced blocks left out.
    fn inline_spans(md: &str) -> Vec<String> {
        let mut prose = String::new();
        let mut inside = false;
        for line in md.lines() {
            if line.trim_start().starts_with("```") {
                inside = !inside;
            } else if !inside {
                prose.push_str(line);
                prose.push('\n');
            }
        }
        prose.split('`').skip(1).step_by(2).map(str::to_string).collect()
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

    /// The rows of the guide's subcommand table: each subcommand with the schema ids its row names.
    fn guide_subcommands(md: &str) -> BTreeMap<String, BTreeSet<String>> {
        let section = md.split("## The subcommands").nth(1).expect("the guide has its subcommand table");
        let section = section.split("\n## ").next().unwrap_or(section);
        section
            .lines()
            .filter_map(|l| l.strip_prefix("| `").map(|rest| (rest.split('`').next().unwrap_or("").to_string(), schema_ids(l))))
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

    /// The comparison behind the row's exit, over any guide and any workflow: `Err` names the first difference.
    fn walk_differs(guide_md: &str, yml: &str) -> Result<Vec<String>, String> {
        let tags: BTreeSet<String> = fences(guide_md).into_iter().map(|(tag, _)| tag).collect();
        if tags != BTreeSet::from(["sh".to_string(), "toml".to_string()]) {
            return Err(format!("a fence that is neither `sh` (compared with the workflow's commands) nor `toml` (compared with its printf): {tags:?}"));
        }
        let mut inside = false;
        for line in guide_md.lines() {
            inside ^= line.trim_start().starts_with("```");
            if !inside && (line.starts_with("    ") || line.starts_with('\t') || line.trim_start().starts_with("~~~")) {
                return Err(format!("an indented or `~~~` code block, which the comparison does not read: {line:?}"));
            }
        }
        let mut guide = fenced(guide_md, "sh");
        for only in GUIDE_ONLY {
            if guide.iter().filter(|l| *l == only).count() != 1 {
                return Err(format!("the stated difference `{only}` is not in the guide exactly once"));
            }
        }
        let second: Vec<String> = guide_steps(guide_md).get(&2).map(|text| fenced(text, "sh")).unwrap_or_default();
        if second.first().map(String::as_str) != Some(GUIDE_ONLY[0]) {
            return Err(format!("the stated difference `{}` is not the first command of step 2, before the install it is for: {second:?}", GUIDE_ONLY[0]));
        }
        guide.retain(|l| !GUIDE_ONLY.contains(&l.as_str()));
        let mut job = workflow_commands(yml);
        for only in WORKFLOW_ONLY {
            if !job.iter().any(|l| l == only) {
                return Err(format!("the workflow does not run `{only}`"));
            }
        }
        job.retain(|l| !WORKFLOW_ONLY.contains(&l.as_str()));
        if guide != job {
            return Err(format!("the guide's ```sh lines {guide:?} are not the one-hour workflow's commands {job:?}"));
        }
        // Inline code is read as a shell line would be, so a wrapper or an assignment before a runner hides nothing.
        // A span the classifier refuses is not a command (`mycelium.tls/1`, `--format text|json`) and is passed over.
        for span in inline_spans(guide_md) {
            let mut segs = Vec::new();
            segments(&span, &mut segs);
            for command in segs.iter().filter_map(|seg| classify(seg).ok().flatten()) {
                if command.contains(' ') && !guide.contains(&command) && !INLINE_ALLOWED.contains(&command.as_str()) {
                    return Err(format!("inline `{span}` is a command outside the walk"));
                }
            }
        }
        Ok(job)
    }

    /// **The row's exit: the guide's commands are the `one-hour` job's.** The guide's `sh` lines, without the one
    /// stated difference ([`GUIDE_ONLY`]), are the commands the workflow runs, without its two environment queries
    /// ([`WORKFLOW_ONLY`]), in the same order — so a step added to either side fails here until the other has it.
    /// The guide has no fence but `sh` and `toml`, no indented or `~~~` block, and no inline code that runs the
    /// binary, pip or Python outside the walk and [`INLINE_ALLOWED`]; the stated difference is step 2's first
    /// command. **Not seen:** a command written in prose without backticks, and a command a workflow reaches through
    /// a file this test does not read — a script or a local action, each of which fails the extraction by name
    /// rather than being read.
    #[test]
    fn the_guides_commands_are_the_one_hour_jobs() {
        let job = walk_differs(FLEET, ONE_HOUR).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(job.len(), 7, "the extraction found the workflow's commands: {job:?}");
        assert!(job.contains(&"mycelium test --faults grant units --config grant.toml".to_string()), "{job:?}");
    }

    /// **Each form, planted, fails** (#770's review, F4). A command added to the real workflow in each form the
    /// first extraction skipped — a single-line `run:`, behind `sudo`, `env`, `time`, `cd … &&`, `!` or `if`, by
    /// its path, in a second `$(…)`, through `pip3` or `uv` — and a command added to the real guide where the first
    /// comparison did not look — a `bash` fence, an untagged one, inline code, one more `sh` line — each makes the
    /// comparison fail, naming the planted command.
    #[test]
    fn a_command_planted_in_any_form_fails_the_comparison() {
        assert!(ONE_HOUR.ends_with('\n') && ONE_HOUR.contains("\n      - name: \"6 · "), "the plants below are indented as the workflow's steps are");
        for line in [
            "sudo mycelium rules",
            "env A=1 mycelium rules",
            "time mycelium rules",
            "cd units && mycelium rules",
            "! mycelium rules",
            "if mycelium rules; then true; fi",
            "/usr/local/bin/mycelium rules",
            "a=$(date) b=$(mycelium rules)",
            "echo \"$(uname -m) $(mycelium rules)\"",
            "pip3 install mycelium-rules",
            "uv pip install mycelium-rules",
        ] {
            for step in [format!("      - name: planted\n        run: |\n          {line}\n"), format!("      - run: {line}\n")] {
                let err = walk_differs(FLEET, &format!("{ONE_HOUR}{step}")).expect_err(line);
                assert!(err.contains("rules\"]"), "`{line}`: the planted command is the workflow's last, and the difference: {err}");
            }
        }
        for (plant, names) in [
            ("\n```bash\nmycelium rules\n```\n", "neither `sh`"),
            ("\n```\nmycelium rules\n```\n", "neither `sh`"),
            ("\nThen run `mycelium rules --format json`.\n", "inline `mycelium rules --format json`"),
            ("\nOr `/usr/local/bin/mycelium rules`.\n", "inline `/usr/local/bin/mycelium rules`"),
            ("\n```sh\nmycelium rules\n```\n", "are not the one-hour workflow's commands"),
        ] {
            let err = walk_differs(&format!("{FLEET}{plant}"), ONE_HOUR).expect_err(plant);
            assert!(err.contains(names), "{plant:?}: {err}");
        }
        let without_venv = FLEET.replace(GUIDE_ONLY[0], "true");
        assert!(walk_differs(&without_venv, ONE_HOUR).is_err(), "the stated difference is required, not merely allowed");
    }

    /// What a comparison over planted texts says: its `Err`, or the message it failed with.
    fn refusal(guide_md: &str, yml: &str) -> String {
        let (guide_md, yml) = (guide_md.to_string(), yml.to_string());
        match std::panic::catch_unwind(move || walk_differs(&guide_md, &yml)) {
            Ok(Ok(job)) => panic!("the plant was passed over: {job:?}"),
            Ok(Err(e)) => e,
            Err(panic) => panic.downcast_ref::<String>().cloned().unwrap_or_else(|| "a panic without a message".into()),
        }
    }

    /// **The confirm round's residues, planted** (#770). In the workflow: a runner held in a variable, a program
    /// that is a variable, a script, a local action, an `env:` entry holding a runner. In the guide: an indented
    /// block, a `~~~` block, inline code behind a wrapper or an assignment, and the virtual-environment line moved
    /// below the install it is for. Each fails, by name.
    #[test]
    fn a_command_reached_indirectly_fails_the_comparison() {
        for (line, names) in [
            ("BIN=/usr/local/bin/mycelium", "a variable that holds a runner"),
            ("PY=python3 true", "a variable that holds a runner"),
            ("\"$BIN\" rules", "a program held in a variable"),
            ("${BIN} rules", "a program held in a variable"),
            ("bash scripts/step.sh", "a script"),
            ("./walk.sh", "a script"),
            ("scripts/walk.py --all", "a script"),
            (". ./env.sh", "a script"),
        ] {
            for step in [format!("      - name: planted\n        run: |\n          {line}\n"), format!("      - run: {line}\n")] {
                let said = refusal(FLEET, &format!("{ONE_HOUR}{step}"));
                assert!(said.contains(names), "`{line}`: {said}");
            }
        }
        let said = refusal(FLEET, &format!("{ONE_HOUR}      - uses: ./.github/actions/walk\n"));
        assert!(said.contains("a local action"), "{said}");
        assert!(ONE_HOUR.contains("\n      PLATFORM: linux-x86_64\n"), "the job's env block, where the entry below is planted");
        let said = refusal(FLEET, &ONE_HOUR.replace("\n      PLATFORM: linux-x86_64\n", "\n      PLATFORM: linux-x86_64\n      BIN: /usr/local/bin/mycelium\n"));
        assert!(said.contains("an env entry that holds"), "{said}");

        for (plant, names) in [
            ("\n    mycelium rules\n", "an indented or `~~~` code block"),
            ("\n~~~\nmycelium rules\n~~~\n", "an indented or `~~~` code block"),
            ("\nOr `sudo mycelium rules`.\n", "inline `sudo mycelium rules`"),
            ("\nOr `X=1 python3 other.py`.\n", "inline `X=1 python3 other.py`"),
            ("\nOr `cd units && mycelium rules`.\n", "inline `cd units && mycelium rules`"),
        ] {
            let said = refusal(&format!("{FLEET}{plant}"), ONE_HOUR);
            assert!(said.contains(names), "{plant:?}: {said}");
        }
        let (venv, init) = (GUIDE_ONLY[0], "mycelium init governed-call-py --template governed-call-py");
        let pip = FLEET.lines().find(|l| l.starts_with("pip install ")).expect("the guide's install line");
        let ordered = format!("{venv}\n{init}\n{pip}\n");
        assert!(FLEET.contains(&ordered), "step 2's three lines, in order");
        let said = refusal(&FLEET.replace(&ordered, &format!("{init}\n{pip}\n{venv}\n")), ONE_HOUR);
        assert!(said.contains("is not the first command of step 2"), "{said}");
    }

    /// The extraction sees a command in every form a workflow can write one — the single-line `run:`, a wrapper
    /// before it, a path-qualified binary, after `cd … &&`, a second `$(…)` on a line, one inside double quotes —
    /// and nothing that only names the binary.
    #[test]
    fn the_workflow_extraction_sees_each_form_of_a_command() {
        let yml = "steps:\n  - name: \"1 · one\"\n    run: sudo mycelium --version\n  - name: \"2 · two\"\n    run: |\n      \
                   set -euo pipefail\n      got=$(mycelium rules --format json)\n      mycelium init \"$HOME/governed-call-py\" --template x\n      \
                   mycelium test --faults grant units --config grant.toml 2> grant.err | tee grant.log || true\n      \
                   MYCELIUM_BIN=mycelium python3 main.py | tee run.log\n      pip install -q \"https://github.com/$REPO/archive/refs/tags/$TAG.tar.gz\"\n      \
                   grep -q 'mycelium explain --closure' recovery.log\n      install -m 0755 a/mycelium /usr/local/bin/mycelium\n      \
                   # mycelium doctor in a comment\n      cd proj && /usr/local/bin/mycelium doctor node.toml > doctor.log\n      \
                   echo \"a $(uname -m) b $(python3 --version)\"; time mycelium wire-check units\n      \
                   if ! env X=1 mycelium declare; then echo no; fi\n      a=$(date) b=$(pip3 list)\n  - name: after\n    run: echo done\n";
        assert_eq!(
            workflow_commands(yml),
            [
                "mycelium --version",
                "mycelium rules --format json",
                "mycelium init governed-call-py --template x",
                "mycelium test --faults grant units --config grant.toml",
                "MYCELIUM_BIN=mycelium python3 main.py",
                "pip install -q \"https://github.com/RichardTathata/mycelium/archive/refs/tags/{{MYCELIUM_TAG}}.tar.gz\"",
                "mycelium doctor node.toml",
                "python3 --version",
                "mycelium wire-check units",
                "X=1 mycelium declare",
                "pip3 list",
            ]
        );
        let steps: Vec<u32> = run_lines(yml).into_iter().map(|(step, _)| step).collect();
        assert_eq!((steps.first(), steps.last(), steps.len()), (Some(&1), Some(&0), 14), "{steps:?}");
    }

    /// **The extraction fails closed.** A line that mentions the binary, pip or Python under a program the test
    /// does not know is a failure, never a skipped line; so is a backtick substitution, which it does not read.
    #[test]
    fn the_workflow_extraction_refuses_what_it_cannot_classify() {
        for line in ["xargs mycelium doctor < configs", "nohup python3 main.py", "uvx pip install x", "echo `mycelium --version`"] {
            let yml = format!("steps:\n  - run: {line}\n");
            assert!(std::panic::catch_unwind(|| workflow_commands(&yml)).is_err(), "`{line}` was passed over");
        }
        assert_eq!(workflow_commands("steps:\n  - run: xargs rm < files\n"), Vec::<String>::new(), "a line that names none of the three is not the extraction's");
    }

    /// **What the guide says a step prints, and writes, is that step's in the workflow — both ways.** Each
    /// expectation the guide quotes under step N is a line step N of the workflow greps for; each line the workflow
    /// greps for is quoted in the guide under the same step or listed in [`NOT_QUOTED`]; and the TOML the guide
    /// gives a step is, line for line, what that step's `printf` appends, in tables the configuration reference has
    /// a row for. The ports the guide names are the scaffold's.
    #[test]
    fn the_guides_expectations_and_configuration_are_the_one_hour_jobs() {
        check_expectations(FLEET, ONE_HOUR);
    }

    /// A grep with other flags is an expectation too: `-Eq` or `-qF` added to step 3 of the real workflow fails the
    /// comparison until the guide quotes it or [`NOT_QUOTED`] lists it. `grep -c`, which this does not read, does not.
    #[test]
    fn a_grep_planted_with_other_flags_fails_the_comparison() {
        let last = "grep -q 'the route needs kv:write' run.log\n";
        assert!(ONE_HOUR.contains(last));
        for (grep, seen) in [("grep -Eq 'a new line' run.log", true), ("grep -qF 'a new line' run.log", true), ("if ! grep -q 'a new line' run.log; then exit 1; fi", true), ("grep -c 'a new line' run.log", false)] {
            let yml = ONE_HOUR.replace(last, &format!("{last}          {grep}\n"));
            let failed = std::panic::catch_unwind(move || check_expectations(FLEET, &yml)).is_err();
            assert_eq!(failed, seen, "`{grep}`");
        }
        let greps = workflow_expectations("steps:\n  - name: setup\n    run: echo x | grep -Eq '^v[0-9]+$'\n  - name: \"1 · one\"\n    run: |\n      grep -qF 'a b' out.log\n      echo x | grep -q \"c d\"\n");
        assert_eq!(greps, BTreeMap::from([(1, BTreeSet::from(["a b".to_string(), "c d".to_string()]))]), "a grep outside the numbered steps is not an expectation of the walk");
    }

    fn check_expectations(guide_md: &str, yml: &str) {
        let (steps, greps, printfs) = (guide_steps(guide_md), workflow_expectations(yml), workflow_configuration(yml));
        assert_eq!(steps.keys().copied().collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5, 6], "the walk's six steps, after its preamble");
        assert_eq!(greps.values().map(BTreeSet::len).sum::<usize>(), 16, "the workflow's greps were found: {greps:?}");
        assert_eq!(greps.keys().copied().collect::<Vec<_>>(), [3, 4, 5, 6]);

        let mut quoted_total = 0;
        for (step, text) in &steps {
            let quoted: BTreeSet<String> = text.lines().filter(|l| l.starts_with("Expect: ")).flat_map(|l| l.split('`').skip(1).step_by(2)).map(str::to_string).collect();
            let grepped = greps.get(step).cloned().unwrap_or_default();
            let not_quoted: BTreeSet<String> = NOT_QUOTED.iter().filter(|(s, _)| s == step).map(|(_, l)| l.to_string()).collect();
            assert!(quoted.is_subset(&grepped), "step {step}: the guide expects {:?}, which the workflow's step does not grep for", quoted.difference(&grepped));
            let unquoted: BTreeSet<String> = grepped.difference(&quoted).cloned().collect();
            assert_eq!(unquoted, not_quoted, "step {step}: what the workflow greps for and the guide does not quote, then NOT_QUOTED");
            quoted_total += quoted.len();

            let toml: Vec<String> = fences(text).into_iter().filter(|(tag, _)| tag == "toml").flat_map(|(_, lines)| lines).collect();
            assert_eq!(&toml, printfs.get(step).unwrap_or(&Vec::new()), "step {step}: the guide's TOML, then the workflow's printf");
            for table in toml.iter().filter_map(|l| l.strip_prefix('[').and_then(|l| l.strip_suffix(']'))) {
                assert!(CONFIGURATION.contains(&format!("\n| {table} |")), "docs/reference/configuration.md has no row for `{table}`");
            }
        }
        assert_eq!(quoted_total, 6, "every step from 3 on says what to expect");
        assert_eq!(printfs.keys().copied().collect::<Vec<_>>(), [4, 5], "steps 4 and 5 write a configuration: {printfs:?}");

        let step3 = &steps[&3];
        assert!(step3.contains("7950") && SCAFFOLD_NODE.contains("bind_port = 7950"), "the mesh port is the scaffold's");
        assert!(step3.contains("8300") && SCAFFOLD_NODE.contains("http_port = 8300"), "the gateway port is the scaffold's");
    }

    /// The features the guide and `AGENTS.md` name are the release's, and the answer the guide says a default build
    /// gives is the one `test --faults` prints.
    #[test]
    fn the_features_named_are_the_releases() {
        let features = RELEASE.lines().find_map(|l| l.trim().strip_prefix("NODE_FEATURES:")).expect("release.yml sets NODE_FEATURES").trim();
        assert!(features.split(',').count() >= 4 && features.contains("compliance"), "{features}");
        assert!(FLEET.contains(&format!("(`{features}`)")), "the guide names the release's features: {features}");
        assert!(ROOT_AGENTS.contains(&format!("    cargo run --features {features} --bin mycelium -- skills get fleet\n")), "AGENTS.md's checkout form builds the release's features: {features}");
        assert!(ROOT_AGENTS.contains("    mycelium skills get fleet\n"), "AGENTS.md's published-binary form");
        assert!(FLEET.contains("`not_in_build — compliance`") && include_str!("faults.rs").contains("\"not_in_build — {feature}\""), "the words are test --faults' own");
    }

    /// **Every subcommand the binary accepts is in the guide, and the guide names no other**: the guide's table, the
    /// arms `main` dispatches and the usage `--help` prints are one set. A subcommand's sub-verbs (`tls ca`,
    /// `skills list`) and flags are **not** compared: the guide names a few, and nothing here holds them.
    #[test]
    fn the_guides_subcommands_are_the_binarys() {
        let guide: BTreeSet<String> = guide_subcommands(FLEET).into_keys().collect();
        let (dispatched, usage) = (dispatched_subcommands(MAIN), usage_subcommands(MAIN));
        assert!(dispatched.contains("wire-check") && dispatched.contains("skills") && dispatched.len() >= 9, "{dispatched:?}");
        assert_eq!(guide, dispatched, "the guide's table, then main's dispatch");
        assert_eq!(usage, dispatched, "the usage, then main's dispatch");
    }

    /// **The schema ids, three ways.** Each id [`CLI_SCHEMAS`] gives a subcommand is a literal in the source named
    /// for it; that set is `cli-json.md`'s and the guide's; and each row of the guide's table names exactly its
    /// subcommand's ids. And the reverse, from the code: every `mycelium.*/N` literal in those sources is one of
    /// the reference's or in [`NOT_CLI_DOCUMENTS`], so a schema constant added to a command's source without a
    /// place in `cli-json.md` fails here.
    #[test]
    fn the_guides_schema_ids_are_the_references_and_the_binarys() {
        let reference = schema_ids(CLI_JSON);
        let mut mapped = BTreeSet::new();
        let rows = guide_subcommands(FLEET);
        for (subcommand, ids) in CLI_SCHEMAS {
            let want: BTreeSet<String> = ids.iter().map(|(id, _)| id.to_string()).collect();
            for (id, source) in ids {
                assert!(source.contains(&format!("\"{id}\"")), "`{id}` is not a literal in the source named for `{subcommand}`");
                for literal in schema_ids(source) {
                    assert!(reference.contains(&literal) || NOT_CLI_DOCUMENTS.contains(&literal.as_str()), "`{literal}`, in the source of `{subcommand}`, is not in cli-json.md");
                }
            }
            assert_eq!(rows.get(subcommand), Some(&want), "the guide's `{subcommand}` row, then its schema ids");
            mapped.extend(want);
        }
        assert_eq!(CLI_SCHEMAS.len(), rows.len(), "a row per subcommand");
        assert_eq!(mapped, reference, "the ids mapped to subcommands, then cli-json.md's");
        assert_eq!(schema_ids(FLEET), reference, "the guide's ids, then cli-json.md's");
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
