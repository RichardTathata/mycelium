//! Tutorial 7 (`docs/guide/tutorials/07-the-first-hour.md`) is the `one-hour` job in prose, step for step: a reader
//! and the job walk one path, and the job's green run on a release is the proof that the tutorial's commands do not
//! fail as written (adopter-surface X11, step 4). This test holds the two together: every fenced block the tutorial
//! tags `one-hour:N` equals the workflow's step `N · …` with the job's bookkeeping removed — the shell options, the
//! skip marker, the clock reads and the CSV row — and every step of the job has its block.

use std::path::Path;

const WORKFLOW: &str = ".github/workflows/one-hour.yml";
const TUTORIAL: &str = "docs/guide/tutorials/07-the-first-hour.md";

/// The job's bookkeeping, which the tutorial does not carry.
fn bookkeeping(line: &str) -> bool {
    line == "set -euo pipefail"
        || line == "[ -f /tmp/skip ] && exit 0"
        || line == "t=$(date +%s)"
        || (line.starts_with("echo \"") && line.contains("/tmp/steps.csv"))
        || line.starts_with('#')
        || line.is_empty()
}

/// The workflow's numbered steps: `(N, the run block's lines, trimmed, bookkeeping removed)`.
fn workflow_steps(yml: &str) -> Vec<(u32, Vec<String>)> {
    let lines: Vec<&str> = yml.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim_start();
        if let Some(rest) = l.strip_prefix("- name: \"")
            && let Some(n) = rest.chars().next().and_then(|c| c.to_digit(10)).filter(|_| rest[1..].starts_with(" · "))
        {
            // Find this step's `run: |` and gather its block: the lines indented deeper than `run:`.
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim_start().starts_with("run: |") && !lines[j].trim_start().starts_with("- name:") {
                j += 1;
            }
            if j < lines.len() && lines[j].trim_start().starts_with("run: |") {
                let run_indent = lines[j].len() - lines[j].trim_start().len();
                let mut block = Vec::new();
                let mut k = j + 1;
                while k < lines.len() {
                    let raw = lines[k];
                    let indent = raw.len() - raw.trim_start().len();
                    if !raw.trim().is_empty() && indent <= run_indent {
                        break;
                    }
                    let t = raw.trim().to_string();
                    if !bookkeeping(&t) {
                        block.push(t);
                    }
                    k += 1;
                }
                out.push((n, block));
                i = k;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The tutorial's tagged blocks: `(N, the fence's lines, trimmed, comments and blanks removed)`.
fn tutorial_blocks(md: &str) -> Vec<(u32, Vec<String>)> {
    let mut out = Vec::new();
    let mut current: Option<(u32, Vec<String>)> = None;
    for line in md.lines() {
        if let Some((n, block)) = current.as_mut() {
            if line.trim_start().starts_with("```") {
                out.push((*n, std::mem::take(block)));
                current = None;
            } else {
                let t = line.trim().to_string();
                if !t.is_empty() && !t.starts_with('#') {
                    block.push(t);
                }
            }
            continue;
        }
        if let Some(info) = line.trim_start().strip_prefix("```")
            && let Some(tag) = info.split_whitespace().find_map(|w| w.strip_prefix("one-hour:"))
        {
            let n: u32 = tag.parse().unwrap_or_else(|_| panic!("a one-hour tag that is not a step number: {info}"));
            current = Some((n, Vec::new()));
        }
    }
    out
}

#[test]
fn the_tutorials_blocks_are_the_jobs_steps() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let yml = std::fs::read_to_string(root.join(WORKFLOW)).expect("the one-hour workflow");
    let md = std::fs::read_to_string(root.join(TUTORIAL)).expect("the tutorial");
    let steps = workflow_steps(&yml);
    let blocks = tutorial_blocks(&md);
    assert!(!steps.is_empty(), "no numbered steps found in {WORKFLOW}");
    assert_eq!(
        steps.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        blocks.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        "the tutorial's tagged blocks are the job's steps, in order"
    );
    for ((n, step), (_, block)) in steps.iter().zip(blocks.iter()) {
        assert!(!step.is_empty(), "step {n} of the job has no commands after its bookkeeping");
        assert_eq!(step, block, "step {n}: the tutorial's block differs from the job's step\n  job:      {step:#?}\n  tutorial: {block:#?}");
    }
    assert_eq!(steps.len(), 6, "the claim lists six steps");
}

#[test]
fn the_parser_strips_only_the_jobs_bookkeeping() {
    let yml = "      - name: \"1 · A step\"\n        if: x\n        run: |\n          set -euo pipefail\n          [ -f /tmp/skip ] && exit 0\n          t=$(date +%s)\n          # a comment\n          curl -fsSLO \"$base/x\"\n          echo \"1-x,$(( $(date +%s) - t ))\" >> /tmp/steps.csv\n      - name: \"The measurement\"\n        run: |\n          echo no\n";
    let steps = workflow_steps(yml);
    assert_eq!(steps, vec![(1, vec!["curl -fsSLO \"$base/x\"".to_string()])]);
    let md = "text\n```bash one-hour:1\n# a comment\ncurl -fsSLO \"$base/x\"\n\n```\n```sh\nTAG=v1\n```\n";
    assert_eq!(tutorial_blocks(md), vec![(1, vec!["curl -fsSLO \"$base/x\"".to_string()])]);
}
