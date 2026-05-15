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
    /// Dim divider line above the virtual-targets cluster. The Go TUI
    /// renders this as `  ─ collectors ─` so the user can tell the
    /// supervisor-managed entries (currently just `otel-errors`)
    /// apart from their own `.run/*.yaml` targets.
    Separator { label: String },
}

/// Compute the visible row list for the current target set + folder
/// expansion state.
///
/// Layout (matches the Go TUI):
///   1. Top-level (group=="") **non-virtual** targets, original order.
///   2. Folder groups in alphabetical order, each header followed by
///      its members when expanded.
///   3. Virtual targets (e.g. `otel-errors`) clustered at the very
///      bottom, preceded by a dim `─ collectors ─` separator so the
///      supervisor-managed entries are visually distinct from
///      `.run/*.yaml` targets.
pub fn compute(targets: &[RunTarget], expanded: &BTreeMap<String, bool>) -> Vec<Row> {
    let mut out = Vec::with_capacity(targets.len());

    // Pass 1: top-level non-virtual targets.
    for (i, t) in targets.iter().enumerate() {
        if t.is_virtual {
            continue;
        }
        if t.group.is_empty() {
            out.push(Row::Target {
                target_idx: i,
                group: String::new(),
            });
        }
    }

    // Pass 2: folder groups, alphabetical for deterministic rendering.
    // Within each group the original target order is preserved.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, t) in targets.iter().enumerate() {
        if t.is_virtual {
            continue;
        }
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

    // Pass 3: virtual targets at the bottom, separator first.
    let virtuals: Vec<usize> = targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_virtual)
        .map(|(i, _)| i)
        .collect();
    if !virtuals.is_empty() {
        out.push(Row::Separator {
            label: "─ collectors ─".to_string(),
        });
        for i in virtuals {
            out.push(Row::Target {
                target_idx: i,
                group: String::new(),
            });
        }
    }

    out
}

/// True if the row is selectable. Separators are skipped by
/// navigation keys.
pub fn is_selectable(r: &Row) -> bool {
    !matches!(r, Row::Separator { .. })
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

    fn virtual_tg(name: &str) -> RunTarget {
        RunTarget {
            name: name.into(),
            is_virtual: true,
            ..Default::default()
        }
    }

    #[test]
    fn virtual_target_pushed_to_bottom_with_separator() {
        let targets = vec![tg("api", ""), tg("worker", ""), virtual_tg("otel-errors")];
        let rows = compute(&targets, &BTreeMap::new());
        // Expected: api, worker, separator, otel-errors.
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert!(matches!(&rows[0], Row::Target { target_idx: 0, .. }));
        assert!(matches!(&rows[1], Row::Target { target_idx: 1, .. }));
        assert!(matches!(&rows[2], Row::Separator { label } if label.contains("collectors")));
        assert!(matches!(&rows[3], Row::Target { target_idx: 2, .. }));
    }

    #[test]
    fn no_separator_when_no_virtual_targets() {
        let targets = vec![tg("api", ""), tg("worker", "")];
        let rows = compute(&targets, &BTreeMap::new());
        assert!(
            rows.iter().all(|r| !matches!(r, Row::Separator { .. })),
            "separator should not appear without a virtual target: {rows:?}"
        );
    }

    #[test]
    fn virtual_target_after_folder_groups() {
        let targets = vec![
            tg("api", ""),
            tg("kb-a", "kb"),
            tg("kb-b", "kb"),
            virtual_tg("otel-errors"),
        ];
        let mut expanded = BTreeMap::new();
        expanded.insert("kb".into(), true);
        let rows = compute(&targets, &expanded);
        // Last row must be the virtual target.
        let last = rows.last().expect("non-empty");
        match last {
            Row::Target { target_idx, .. } => assert_eq!(*target_idx, 3),
            other => panic!("last row not the virtual target: {other:?}"),
        }
        // The separator sits directly above it.
        let sep_idx = rows.len() - 2;
        assert!(matches!(&rows[sep_idx], Row::Separator { .. }));
    }

    #[test]
    fn is_selectable_skips_separator() {
        assert!(!is_selectable(&Row::Separator { label: "x".into() }));
        assert!(is_selectable(&Row::Target {
            target_idx: 0,
            group: String::new()
        }));
        assert!(is_selectable(&Row::Folder {
            group: "x".into(),
            expanded: false,
            count: 1
        }));
    }
}
