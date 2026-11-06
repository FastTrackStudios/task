//! Task's layer on architect's vault.
//!
//! The vault (notes, properties, links, tags, `.base` views) is
//! architect's since FastTrackStudios/architect#333. What is Task's own
//! stays here and is handed to it through the vault's hooks:
//!
//! - the `area` and `task` kinds, inserted into
//!   `vault_live::property_schema::PropertySchemaRegistry` on top of its
//!   built-ins (`node`, `person`, `daily`, `project`): [`schema_registry`];
//! - the `.base` formula `transpose_key(key, offset)`, registered with
//!   `vault_live::bases::register_formula_fn`: [`install`].
//!
//! `project` is architect's (FastTrackStudios/architect#344), and so is
//! `project.md` as the marker that makes a folder a root of its own: the
//! vault's default, which `org_proto::PROJECT_PAGE` names too.
//!
//! Call [`install`] once before a base is evaluated;
//! `task_server::AppState::open_at` and the CLI's `main` do.

use vault_live::property_schema::{
    EnumOption, KindSchema, PropertyDef, PropertySchemaRegistry, PropertyType,
};

/// Registers `transpose_key` with every base. Safe to call more than
/// once: the formula is replaced by itself.
pub fn install() {
    vault_live::bases::register_formula_fn("transpose_key", transpose_key);
}

/// The registry with the vault's built-ins (`project` among them) and
/// Task's kinds.
#[must_use]
pub fn schema_registry() -> PropertySchemaRegistry {
    let mut r = PropertySchemaRegistry::with_builtins();
    for s in kinds() {
        r.insert(s);
    }
    r
}

/// Task's kinds: `area` and `task`, each extending `node`.
#[must_use]
pub fn kinds() -> Vec<KindSchema> {
    vec![area_schema(), task_schema()]
}

/// `transpose_key(key, offset)`: a key name moved by semitones (`2`,
/// `"+4"`, `"-1"`) or replaced by an absolute key (`"D"`), in the input's
/// own spelling (`music_key::transpose_key`).
///
/// A missing or empty offset means "no change", so a setlist entry
/// without `key_offset` shows the song's key. Another arity, a
/// non-integer number or an unparseable key or offset is `null`, like
/// every other type mismatch in a base. An absolute key offset is
/// returned as written, whatever `key` holds.
#[must_use]
pub fn transpose_key(args: &[serde_json::Value]) -> serde_json::Value {
    use serde_json::Value as V;
    let [key, offset] = args else {
        return V::Null;
    };
    let offset = match offset {
        V::Null => "0".to_string(),
        V::String(s) if s.trim().is_empty() => "0".to_string(),
        V::String(s) => s.clone(),
        V::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(f)) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
            _ => return V::Null,
        },
        _ => return V::Null,
    };
    let key = key.as_str().unwrap_or("");
    music_key::transpose_key(key, &offset).map_or(V::Null, V::String)
}

pub fn area_schema() -> KindSchema {
    KindSchema {
        kind: "area".into(),
        extends: Some("node".into()),
        properties: vec![
            PropertyDef {
                key: "up".into(),
                ty: PropertyType::LinkList,
                required: false,
                default: None,
                display_name: Some("Up".into()),
                icon: Some("arrow-up".into()),
            },
            PropertyDef {
                key: "icon".into(),
                ty: PropertyType::Text,
                required: false,
                default: None,
                display_name: Some("Icon".into()),
                icon: None,
            },
            PropertyDef {
                key: "tags".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Tags".into()),
                icon: Some("tag".into()),
            },
        ],
    }
}

pub fn task_schema() -> KindSchema {
    KindSchema {
        kind: "task".into(),
        extends: Some("node".into()),
        properties: vec![
            PropertyDef {
                key: "title".into(),
                ty: PropertyType::Text,
                required: true,
                default: None,
                display_name: Some("Title".into()),
                icon: None,
            },
            PropertyDef {
                key: "status".into(),
                ty: PropertyType::EnumWithMetadata {
                    options: vec![
                        EnumOption {
                            value: "todo".into(),
                            label: "To do".into(),
                            color: Some("muted".into()),
                            icon: Some("circle".into()),
                            order: 0,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "in_progress".into(),
                            label: "In progress".into(),
                            color: Some("primary".into()),
                            icon: Some("circle-dot".into()),
                            order: 1,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "blocked".into(),
                            label: "Blocked".into(),
                            color: Some("warning".into()),
                            icon: Some("octagon-x".into()),
                            order: 2,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: None,
                        },
                        EnumOption {
                            value: "done".into(),
                            label: "Done".into(),
                            color: Some("success".into()),
                            icon: Some("circle-check".into()),
                            order: 3,
                            is_completed: true,
                            auto_archive_delay_days: Some(7),
                            weight: None,
                        },
                    ],
                },
                required: false,
                default: Some(serde_json::Value::String("todo".into())),
                display_name: Some("Status".into()),
                icon: Some("circle".into()),
            },
            PropertyDef {
                key: "priority".into(),
                ty: PropertyType::EnumWithMetadata {
                    options: vec![
                        EnumOption {
                            value: "low".into(),
                            label: "Low".into(),
                            color: Some("muted".into()),
                            icon: None,
                            order: 0,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(0),
                        },
                        EnumOption {
                            value: "normal".into(),
                            label: "Normal".into(),
                            color: Some("primary".into()),
                            icon: None,
                            order: 1,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(10),
                        },
                        EnumOption {
                            value: "high".into(),
                            label: "High".into(),
                            color: Some("warning".into()),
                            icon: Some("chevron-up".into()),
                            order: 2,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(20),
                        },
                        EnumOption {
                            value: "urgent".into(),
                            label: "Urgent".into(),
                            color: Some("danger".into()),
                            icon: Some("flame".into()),
                            order: 3,
                            is_completed: false,
                            auto_archive_delay_days: None,
                            weight: Some(30),
                        },
                    ],
                },
                required: false,
                default: Some(serde_json::Value::String("normal".into())),
                display_name: Some("Priority".into()),
                icon: None,
            },
            PropertyDef {
                key: "due".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Due".into()),
                icon: Some("calendar".into()),
            },
            PropertyDef {
                key: "scheduled".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Scheduled".into()),
                icon: Some("calendar-clock".into()),
            },
            PropertyDef {
                key: "completed_date".into(),
                ty: PropertyType::Date,
                required: false,
                default: None,
                display_name: Some("Completed".into()),
                icon: None,
            },
            PropertyDef {
                key: "tags".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Tags".into()),
                icon: Some("tag".into()),
            },
            PropertyDef {
                key: "contexts".into(),
                ty: PropertyType::Tags,
                required: false,
                default: None,
                display_name: Some("Contexts".into()),
                icon: Some("at-sign".into()),
            },
            PropertyDef {
                key: "projects".into(),
                ty: PropertyType::LinkList,
                required: false,
                default: None,
                display_name: Some("Projects".into()),
                icon: Some("folder".into()),
            },
            PropertyDef {
                key: "blocked_by".into(),
                ty: PropertyType::StructList {
                    fields: vec![
                        PropertyDef {
                            key: "uid".into(),
                            ty: PropertyType::Link,
                            required: true,
                            default: None,
                            display_name: Some("Blocker".into()),
                            icon: None,
                        },
                        PropertyDef {
                            key: "reltype".into(),
                            ty: PropertyType::Text,
                            required: false,
                            default: Some(serde_json::Value::String("FINISHTOSTART".into())),
                            display_name: Some("Relation".into()),
                            icon: None,
                        },
                        PropertyDef {
                            key: "gap".into(),
                            ty: PropertyType::Text,
                            required: false,
                            default: None,
                            display_name: Some("Gap (ISO 8601)".into()),
                            icon: None,
                        },
                    ],
                },
                required: false,
                default: None,
                display_name: Some("Blocked by".into()),
                icon: Some("ban".into()),
            },
            PropertyDef {
                key: "time_estimate".into(),
                ty: PropertyType::Number,
                required: false,
                default: None,
                display_name: Some("Estimate (min)".into()),
                icon: Some("hourglass".into()),
            },
            PropertyDef {
                key: "sort_order".into(),
                ty: PropertyType::LexoRank,
                required: false,
                default: None,
                display_name: None,
                icon: None,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;
    use vault_live::bases::{BaseEval, BaseRow, expr_parser};

    use super::*;

    fn row(name: &str, fm: serde_json::Value) -> BaseRow {
        BaseRow::from_parts(Uuid::new_v4(), name, &fm.to_string())
    }

    fn page(path: &str, fm: serde_json::Value) -> BaseRow {
        let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
        BaseRow::from_parts_full(
            Uuid::new_v4(),
            file.trim_end_matches(".md").to_string(),
            path,
            folder.to_string(),
            "md",
            &fm.to_string(),
            &[],
        )
    }

    #[test]
    fn the_registry_has_the_builtins_and_tasks_kinds() {
        let r = schema_registry();
        for kind in ["node", "person", "daily", "area", "project", "task"] {
            assert!(r.get(kind).is_some(), "missing {kind} schema");
        }
        // `project` is the vault's own, not replaced by Task.
        let project = r.get("project").expect("project schema");
        assert!(project.properties.iter().any(|p| p.key == "state"));
    }

    #[test]
    fn transpose_key_values() {
        let cases = [
            (json!("f#m"), json!(2), json!("g#m")),
            (json!("D"), json!("+4"), json!("F#")),
            (json!("D"), json!("-4"), json!("Bb")),
            (json!("Eb"), json!(3), json!("Gb")),
            (json!("Bb minor"), json!(-1), json!("A minor")),
            (json!("G"), json!(4.0), json!("B")),
            // Absolute key: passed through unchanged.
            (json!("C"), json!("D"), json!("D")),
            (json!(null), json!("Ebm"), json!("Ebm")),
            // No offset: the key itself.
            (json!("Ab"), json!(null), json!("Ab")),
            (json!("Ab"), json!(""), json!("Ab")),
            // Bad input: null.
            (json!("H"), json!(2), json!(null)),
            (json!("Cmaj7"), json!(2), json!(null)),
            (json!("C"), json!(1.5), json!(null)),
            (json!("C"), json!("four"), json!(null)),
            (json!("C"), json!(true), json!(null)),
            (json!(null), json!(2), json!(null)),
        ];
        for (key, offset, want) in cases {
            assert_eq!(
                transpose_key(&[key.clone(), offset.clone()]),
                want,
                "transpose_key({key}, {offset})"
            );
        }
        // Arity is checked.
        assert_eq!(transpose_key(&[json!("C")]), json!(null));
    }

    #[test]
    fn transpose_key_reads_properties_and_links_in_a_filter() {
        install();
        let rows = vec![
            row(
                "1 Opener",
                json!({ "kind": "use", "key": "D", "key_offset": 4 }),
            ),
            row(
                "2 Ballad",
                json!({ "kind": "use", "key": "Bb", "key_offset": "D" }),
            ),
            row(
                "3 Closer",
                json!({ "kind": "use", "key": "f#m", "key_offset": "+2" }),
            ),
            row("4 Default", json!({ "kind": "use", "key": "F#" })),
        ];
        let f = expr_parser::parse_filter(r#"transpose_key(key, key_offset) == "F#""#).unwrap();
        let hits: Vec<_> = rows
            .iter()
            .filter(|r| BaseEval::over(&[]).filter_matches(&f, r))
            .map(|r| r.basename.clone())
            .collect();
        assert_eq!(hits, vec!["1 Opener", "4 Default"]);

        // The setlist shape: the use links its song, the key lives there.
        let song = page("Songs/Night Drive.md", json!({ "key": "D" }));
        let usage = page(
            "Gigs/1 Opener.md",
            json!({ "song": "[[Night Drive]]", "key_offset": 4 }),
        );
        let all = vec![song, usage.clone()];
        let f = expr_parser::parse_filter(r#"transpose_key(note.song.key, key_offset) == "F#""#)
            .unwrap();
        assert!(BaseEval::over(&all).filter_matches(&f, &usage));
    }

    #[test]
    fn a_project_page_makes_a_shelf_without_task_setting_it() {
        // Task's org layer (`org_proto::walk_projects`) and the vault's
        // walk agree on what a project is.
        assert_eq!(
            vault_live::shelf_boundary::PROJECT_PAGE,
            org_proto::PROJECT_PAGE
        );
        assert_eq!(
            vault_live::shelf_boundary::shelf_markers(),
            &[org_proto::PROJECT_PAGE]
        );
    }
}
