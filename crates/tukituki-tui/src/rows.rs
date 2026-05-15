//! Sidebar row computation.
//!
//! The sidebar shows a flat list of folder headers + target rows.
//! Targets grouped under a folder (`RunTarget.group != ""`) collapse
//! under a `▶` header; the user toggles individual folders open with
//! Enter/Space/→. This module turns the target list into the visible
//! row sequence given the current expand-state map, and provides
//! lookups for the App's selection cursor.

use std::collections::BTreeMap;

use tukituki_config::RunTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A folder header. `group` is the directory name. `expanded` lets
    /// the renderer pick the glyph (▶ vs ▼). `count` is the number of
    /// targets in this folder.
    Folder {
        group: String,
        expanded: bool,
        count: usize,
    },
    /// A target row. `target_idx` indexes into the original target list.
    /// `group` is the folder this target belongs to (`""` = top-level).
    Target { target_idx: usize, group: String },
}

/// Compute the visible row list for the current target set + folder
/// expansion state. Targets at the top level (`group==""`) are listed
/// before any folder groups; within each section the order is
/// preserved from the input slice — which `load_targets` already sorts
/// by name. Multiple targets sharing the same non-empty group collapse
/// into a single header (followed by member targets when expanded).
pub fn compute(targets: &[RunTarget], expanded: &BTreeMap<String, bool>) -> Vec<Row> {
    let mut out = Vec::with_capacity(targets.len());

    // Top-level first.
    for (i, t) in targets.iter().enumerate() {
        if t.group.is_empty() {
            out.push(Row::Target {
                target_idx: i,
                group: String::new(),
            });
        }
    }

    // Then each non-empty group, in alphabetical order for deterministic
    // rendering. Within a group the original target order is preserved.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, t) in targets.iter().enumerate() {
        if !t.group.is_empty() {
            groups.entry(t.group.clone()).or_default().push(i);
        }
    }
    for (group, idxs) in groups {
        let is_open = expanded.get(&group).copied().unwrap_or(false);
        out.push(Row::Folder {
            group: group.clone(),
            expanded: is_open,
            count: idxs.len(),
        });
        if is_open {
            for i in idxs {
                out.push(Row::Target {
                    target_idx: i,
                    group: group.clone(),
                });
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tg(name: &str, group: &str) -> RunTarget {
        RunTarget {
            name: name.into(),
            group: group.into(),
            ..Default::default()
        }
    }

    #[test]
    fn top_level_only() {
        let targets = vec![tg("a", ""), tg("b", "")];
        let rows = compute(&targets, &BTreeMap::new());
        assert_eq!(
            rows,
            vec![
                Row::Target {
                    target_idx: 0,
                    group: String::new()
                },
                Row::Target {
                    target_idx: 1,
                    group: String::new()
                }
            ]
        );
    }

    #[test]
    fn collapsed_folder_hides_targets() {
        let targets = vec![tg("api", ""), tg("kb-acme", "kb"), tg("kb-sentinel", "kb")];
        let rows = compute(&targets, &BTreeMap::new());
        assert_eq!(
            rows,
            vec![
                Row::Target {
                    target_idx: 0,
                    group: String::new()
                },
                Row::Folder {
                    group: "kb".into(),
                    expanded: false,
                    count: 2
                },
            ]
        );
    }

    #[test]
    fn expanded_folder_shows_targets() {
        let targets = vec![tg("api", ""), tg("kb-acme", "kb"), tg("kb-sentinel", "kb")];
        let mut expanded = BTreeMap::new();
        expanded.insert("kb".into(), true);
        let rows = compute(&targets, &expanded);
        assert_eq!(
            rows,
            vec![
                Row::Target {
                    target_idx: 0,
                    group: String::new()
                },
                Row::Folder {
                    group: "kb".into(),
                    expanded: true,
                    count: 2
                },
                Row::Target {
                    target_idx: 1,
                    group: "kb".into()
                },
                Row::Target {
                    target_idx: 2,
                    group: "kb".into()
                },
            ]
        );
    }

    #[test]
    fn folder_groups_sorted_alphabetically() {
        let targets = vec![
            tg("z-target", "zzz"),
            tg("a-target", "aaa"),
            tg("toplevel", ""),
        ];
        let mut expanded = BTreeMap::new();
        expanded.insert("aaa".into(), true);
        expanded.insert("zzz".into(), true);
        let rows = compute(&targets, &expanded);
        // Top-level row first.
        let Row::Target { target_idx, .. } = &rows[0] else {
            panic!("first row must be a target: {:?}", rows[0]);
        };
        assert_eq!(*target_idx, 2);
        // Then aaa, then zzz.
        match &rows[1] {
            Row::Folder { group, .. } => assert_eq!(group, "aaa"),
            other => panic!("expected aaa folder, got {other:?}"),
        }
        match &rows[3] {
            Row::Folder { group, .. } => assert_eq!(group, "zzz"),
            other => panic!("expected zzz folder, got {other:?}"),
        }
    }
}
