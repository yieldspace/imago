use std::fmt::Debug;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachableGraphEdge<A> {
    pub action: A,
    pub target: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachableGraphSnapshot<S, A> {
    pub states: Vec<S>,
    pub edges: Vec<Vec<ReachableGraphEdge<A>>>,
    pub initial_indices: Vec<usize>,
    pub deadlocks: Vec<usize>,
    pub truncated: bool,
    pub stutter_omitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphState {
    pub summary: String,
    pub full: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphEdge {
    pub label: String,
    pub target: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphSnapshot {
    pub states: Vec<DocGraphState>,
    pub edges: Vec<Vec<DocGraphEdge>>,
    pub initial_indices: Vec<usize>,
    pub deadlocks: Vec<usize>,
    pub truncated: bool,
    pub stutter_omitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphCase {
    pub label: String,
    pub graph: DocGraphSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphSpec {
    pub spec_name: String,
    pub cases: Vec<DocGraphCase>,
}

pub trait DocGraphProvider {
    fn spec_name(&self) -> &'static str;

    fn cases(&self) -> Vec<DocGraphCase>;
}

pub struct RegisteredDocGraphProvider {
    pub spec_name: &'static str,
    pub build: fn() -> Box<dyn DocGraphProvider>,
}

inventory::collect!(RegisteredDocGraphProvider);

pub fn summarize_doc_graph_state<T>(state: &T) -> DocGraphState
where
    T: Debug,
{
    let full = format!("{state:#?}");
    DocGraphState {
        summary: summarize_doc_graph_text(&full),
        full,
    }
}

pub fn summarize_doc_graph_text(input: &str) -> String {
    const MAX_LINES: usize = 8;
    const MAX_CHARS: usize = 240;

    let mut lines = Vec::new();
    let mut total_chars = 0usize;

    for raw_line in input.lines() {
        if lines.len() == MAX_LINES {
            break;
        }

        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let available = MAX_CHARS.saturating_sub(total_chars);
        if available == 0 {
            break;
        }

        let mut line = trimmed.chars().take(available).collect::<String>();
        let raw_len = trimmed.chars().count();
        total_chars += line.chars().count();
        if raw_len > line.chars().count() {
            if !line.ends_with("...") {
                line.push_str("...");
            }
            lines.push(line);
            return lines.join("\n");
        }

        lines.push(line);
    }

    if lines.is_empty() {
        return input.trim().chars().take(MAX_CHARS).collect();
    }

    let consumed_lines = input.lines().filter(|line| !line.trim().is_empty()).count();
    let consumed_chars = lines.iter().map(|line| line.chars().count()).sum::<usize>();
    if (consumed_lines > lines.len() || input.chars().count() > consumed_chars)
        && let Some(last) = lines.last_mut()
        && !last.ends_with("...")
    {
        if last.chars().count() + 3 > MAX_CHARS && !last.is_empty() {
            let mut shortened = last
                .chars()
                .take(last.chars().count().saturating_sub(3))
                .collect::<String>();
            shortened.push_str("...");
            *last = shortened;
        } else {
            last.push_str("...");
        }
    }

    lines.join("\n")
}

pub fn collect_doc_graph_specs() -> Vec<DocGraphSpec> {
    let mut specs = inventory::iter::<RegisteredDocGraphProvider>
        .into_iter()
        .map(|entry| {
            let provider = (entry.build)();
            DocGraphSpec {
                spec_name: provider.spec_name().to_owned(),
                cases: provider.cases(),
            }
        })
        .collect::<Vec<_>>();
    specs.sort_by(|left, right| left.spec_name.cmp(&right.spec_name));
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_doc_graph_state_preserves_full_and_shortens_summary() {
        #[derive(Debug)]
        struct DemoState {
            phase: &'static str,
            counter: u32,
            notes: &'static str,
        }

        let state = DemoState {
            phase: "Listening",
            counter: 7,
            notes: "A long note that should still remain visible in the full debug representation",
        };
        let _ = (&state.phase, state.counter, &state.notes);
        let summarized = summarize_doc_graph_state(&state);

        assert!(summarized.full.contains("phase: \"Listening\""));
        assert!(summarized.summary.contains("phase: \"Listening\""));
        assert!(summarized.summary.lines().count() <= 8);
        assert!(summarized.summary.len() <= 243);
    }

    #[test]
    fn summarize_doc_graph_text_truncates_large_payloads() {
        let text = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10";
        let summarized = summarize_doc_graph_text(text);

        assert!(summarized.contains("line1"));
        assert!(summarized.contains("line8"));
        assert!(!summarized.contains("line10"));
        assert!(summarized.ends_with("..."));
    }
}
