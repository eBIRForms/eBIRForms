//! Pins how the assistant writes each official box
//! (`bir_core::forms::agent`). The bindings are derived by probing these
//! sample drafts (`agent::probe`), never typed by hand; the boxes come from
//! `rules/forms/<package>/fields.json`.
//!
//! Regenerate after a form, sample or fields.json change with
//! `UPDATE_AGENT_FORMS=1 cargo test -p bir-core --test sample_payloads_test agent_forms`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bir_core::forms::agent::{self, AgentFormDef, AgentFormKind, Binding, probe};
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `fields.json` inventories this test reads, copied from the source
/// repo's `rules/forms/`.
fn rules_forms() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules-forms")
}

fn data_path(package: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data/agent-forms")
        .join(format!("{package}.json"))
}

/// The dummy profile's own strings: a draft field holding one of them is
/// the profile's.
fn profile_values() -> Vec<String> {
    let profile = crate::dummy_profile("", "Individual");
    let tin = profile.tin.full();
    vec![
        profile.full_name.clone(),
        profile.registered_address.clone(),
        profile.zip_code.clone(),
        profile.phone.clone(),
        profile.email.clone(),
        profile.email.to_uppercase(),
        profile.rdo_code.clone(),
        profile.line_of_business.clone(),
        tin.clone(),
        format!("{}-{}-{}-{}", &tin[0..3], &tin[3..6], &tin[6..9], &tin[9..]),
    ]
}

/// One entry per line, so a change shows as a one-line diff.
fn pinned_text(def: &AgentFormDef) -> String {
    let mut value = serde_json::to_value(def).expect("definition serializes");
    // Workspace dependencies can enable serde_json's preserve_order feature.
    value.sort_all_objects();
    let line = |value: &Value| serde_json::to_string(value).expect("json");
    let mut out = String::from("{\n");
    let object = value.as_object().expect("object");
    let fields: Vec<String> = object
        .iter()
        .map(|(key, child)| match child {
            Value::Array(items) if !items.is_empty() => format!(
                "\"{key}\": [\n{}\n]",
                items.iter().map(line).collect::<Vec<_>>().join(",\n")
            ),
            Value::Object(map) if !map.is_empty() => format!(
                "\"{key}\": {{\n{}\n}}",
                map.iter()
                    .map(|(name, item)| format!(
                        "{}: {}",
                        line(&Value::from(name.as_str())),
                        line(item)
                    ))
                    .collect::<Vec<_>>()
                    .join(",\n")
            ),
            other => format!("\"{key}\": {}", line(other)),
        })
        .collect();
    out.push_str(&fields.join(",\n"));
    out.push_str("\n}\n");
    out
}

#[test]
fn pinned_agent_form_serialization_is_canonical() {
    for kind in AgentFormKind::all() {
        let pinned = std::fs::read_to_string(data_path(kind.package())).expect("pinned form reads");
        assert_eq!(pinned_text(kind.definition()), pinned, "{}", kind.package());
    }
}

fn derived(kind: AgentFormKind, seeds: &[Value]) -> AgentFormDef {
    let fields = rules_forms().join(kind.package()).join("fields.json");
    let raw = std::fs::read_to_string(&fields)
        .unwrap_or_else(|error| panic!("{}: {error}", fields.display()));
    let mut boxes = probe::catalog(&raw).expect("fields.json projects");
    let extra = probe::supplement_from_field_map(kind, seeds, &boxes);
    boxes.extend(extra);
    let rows = probe::supplement_row_families(kind, seeds, &boxes);
    boxes.extend(rows);
    probe::apply_labels(&mut boxes, captions());
    probe::supplement_app_radios(kind, &mut boxes);
    probe::derive(kind, seeds, &profile_values(), boxes)
}

/// Writer key -> the caption printed beside its box on the official sheet,
/// from every `html-frozen/<sheet>/writer-cells.json` (at the repo root, or
/// under `crates/bir-print/` once the sheets move there).
fn captions() -> &'static BTreeMap<String, String> {
    static CAPTIONS: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    CAPTIONS.get_or_init(|| {
        let mut captions = BTreeMap::new();
        for root in [repo_root().join("crates/bir-print/assets/html-frozen")] {
            let Ok(sheets) = std::fs::read_dir(&root) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = sheets
                .filter_map(Result::ok)
                .map(|sheet| sheet.path().join("writer-cells.json"))
                .filter(|path| path.is_file())
                .collect();
            paths.sort();
            for path in paths {
                let cells: Value = serde_json::from_str(
                    &std::fs::read_to_string(&path).expect("writer-cells.json reads"),
                )
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                for list in ["joins", "money_joins", "xbox_joins"] {
                    for cell in cells[list].as_array().into_iter().flatten() {
                        if let (Some(key), Some(printed)) =
                            (cell["writer_key"].as_str(), cell["printed"].as_str())
                            && !printed.trim().is_empty()
                        {
                            captions
                                .entry(key.to_string())
                                .or_insert_with(|| printed.trim().to_string());
                        }
                    }
                }
            }
        }
        assert!(
            !captions.is_empty(),
            "no html-frozen writer-cells.json found"
        );
        captions
    })
}

fn seeds_for(kind: AgentFormKind) -> Vec<Value> {
    let module = kind.form_code().to_ascii_lowercase();
    super::agent_seeds()
        .into_iter()
        .find(|(name, _)| *name == module)
        .map(|(_, seeds)| seeds)
        .unwrap_or_else(|| panic!("no sample module for {}", kind.form_code()))
}

#[test]
fn agent_forms_are_derived_from_the_samples() {
    let update = std::env::var_os("UPDATE_AGENT_FORMS").is_some();
    let seeds: Vec<(AgentFormKind, Vec<Value>)> = AgentFormKind::all()
        .into_iter()
        .map(|kind| (kind, seeds_for(kind)))
        .collect();
    // Probing is independent per form; spread it over the cores.
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let chunk = seeds.len().div_ceil(threads);
    let defs: Vec<(AgentFormKind, AgentFormDef)> = std::thread::scope(|scope| {
        let handles: Vec<_> = seeds
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|(kind, seeds)| (*kind, derived(*kind, seeds)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("probe thread"))
            .collect()
    });
    let mut stale = Vec::new();
    for (kind, def) in defs {
        // Every user box either maps or says why it cannot.
        for entry in def.boxes.iter().filter(|entry| entry.is_user_box()) {
            assert!(
                def.bindings.contains_key(&entry.key) || def.unmapped.contains_key(&entry.key),
                "{} {}: neither bound nor listed as unmappable",
                kind.form_code(),
                entry.key
            );
        }
        let text = pinned_text(&def);
        let path = data_path(kind.package());
        if update {
            std::fs::write(&path, &text).expect("write pinned agent form");
        } else {
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                stale.push(kind.package());
            }
            // The pinned file is what the app loads.
            assert_eq!(kind.definition(), &def, "{}", kind.package());
        }
    }
    assert!(
        stale.is_empty(),
        "agent form data is stale for {stale:?}; run UPDATE_AGENT_FORMS=1 cargo test -p bir-core --test sample_payloads_test agent_forms"
    );
}

/// Filling a bound box through `agent::fill` shows the value in the
/// official box, for every value box of the first sample of each form.
#[test]
fn filling_a_box_shows_its_value_in_the_official_box() {
    for kind in AgentFormKind::all() {
        let def = kind.definition();
        let seed = seeds_for(kind).remove(0);
        let base_map = agent::field_map(kind, &seed).expect("sample maps");
        let mut checked = 0;
        for entry in def.fillable() {
            let bound = &def.bindings[&entry.key];
            let (value, expected): (Value, &str) = match &bound.binding {
                Binding::Amount { .. } => (Value::from("4,321.17"), "4321"),
                Binding::Text { date: None, .. } => (Value::from("7193"), "7193"),
                _ => continue,
            };
            // Boxes the sample does not show yet (other rows, gated items)
            // are covered by the probe itself.
            if !base_map.contains_key(&entry.key) {
                continue;
            }
            // An ATC box only keeps codes from its table; `7193` is not one.
            if entry.key.to_ascii_uppercase().contains("ATC")
                || entry.label.to_ascii_uppercase().contains("ATC")
            {
                continue;
            }
            let mut draft = seed.clone();
            if agent::fill(kind, &mut draft, &entry.key, &value, None).is_err() {
                continue;
            }
            agent::compute(kind, &mut draft).expect("recompute");
            let map = agent::field_map(kind, &draft).expect("map");
            let shown = map.get(&entry.key).cloned().unwrap_or_default();
            // A box behind an unanswered item (Amended Return = No) keeps
            // its value; anything else must show what was filled.
            let gated = map.get(&entry.key) == base_map.get(&entry.key);
            assert!(
                gated
                    || shown
                        .replace(',', "")
                        .to_ascii_uppercase()
                        .contains(expected),
                "{} {} shows `{shown}` after filling {value}",
                kind.form_code(),
                entry.key
            );
            checked += 1;
        }
        assert!(
            checked > 0 || def.fillable().next().is_none(),
            "{}: no fillable value box was checked",
            kind.form_code()
        );
    }
}

/// Per-form coverage, printed with `--nocapture` for the report.
#[test]
fn agent_form_coverage_summary() {
    let mut lines = Vec::new();
    for kind in AgentFormKind::all() {
        let def = kind.definition();
        let user: Vec<_> = def
            .boxes
            .iter()
            .filter(|e| e.is_user_box() && !e.layout_only)
            .collect();
        let layout_only = def.boxes.iter().filter(|e| e.layout_only).count();
        let fillable_layout = def.fillable().filter(|e| e.layout_only).count();
        let fillable = def.fillable().filter(|e| !e.layout_only).count();
        let mut locked: BTreeMap<String, usize> = BTreeMap::new();
        for entry in &user {
            if let Some(lock) = def.bindings.get(&entry.key).and_then(|bound| bound.lock) {
                *locked.entry(format!("{lock:?}")).or_default() += 1;
            }
        }
        let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
        for (key, reason) in &def.unmapped {
            if def.box_def(key).is_some_and(|e| e.layout_only) {
                continue;
            }
            let head = reason.split(':').next().unwrap_or(reason).to_string();
            *reasons.entry(head).or_default() += 1;
        }
        lines.push(format!(
            "{} user={} fillable={} locked={:?} unmapped={} {:?} layout_only={} (fillable {})",
            kind.form_code(),
            user.len(),
            fillable,
            locked,
            reasons.values().sum::<usize>(),
            reasons,
            layout_only,
            fillable_layout
        ));
    }
    println!("{}", lines.join("\n"));
}

/// Every fillable box has a readable label: `fields.json`'s own, the
/// caption printed beside it on the sheet, or `Item N`; only a box with none
/// of those is named from its key's own words. Those are listed here.
#[test]
fn fillable_boxes_are_labelled() {
    let mut from_key = Vec::new();
    for kind in AgentFormKind::all() {
        let def = kind.definition();
        let fields = rules_forms().join(kind.package()).join("fields.json");
        let raw = probe::catalog(&std::fs::read_to_string(fields).unwrap()).unwrap();
        for entry in def.fillable() {
            assert_ne!(entry.label, entry.key, "{} {}", kind.form_code(), entry.key);
            let Some(original) = raw.iter().find(|raw| raw.key == entry.key) else {
                continue;
            };
            if original.label == original.key {
                if captions().contains_key(&entry.key) {
                    assert_eq!(&entry.label, &captions()[&entry.key], "{}", entry.key);
                } else if !original.item.is_empty() {
                    assert_eq!(
                        entry.label,
                        format!("Item {}", original.item),
                        "{}",
                        entry.key
                    );
                } else {
                    from_key.push(format!(
                        "{} {} -> {}",
                        kind.form_code(),
                        entry.key,
                        entry.label
                    ));
                }
            }
        }
    }
    println!("fillable boxes with no caption or item, named from their key: {from_key:#?}");
}

/// A radio item is one box: its group key, with an option per radio.
#[test]
fn radio_items_are_one_box_with_options() {
    let def = AgentFormKind::from_code("0619E").unwrap().definition();
    let group = def.box_def("frm0619E:optWithheld").expect("item 4 group");
    assert!(group.radio_group);
    let options: Vec<(&str, &str)> = group
        .options
        .iter()
        .map(|option| (option.value.as_str(), option.label.as_str()))
        .collect();
    assert_eq!(options, [("Y", "Yes"), ("N", "No")]);
    assert_eq!(group.label, "4 Any Taxes Withheld?");
    let listed: Vec<&str> = def.fillable().map(|entry| entry.key.as_str()).collect();
    assert!(listed.contains(&"frm0619E:optWithheld"));
    assert!(
        !listed.contains(&"frm0619E:optWithheld:Y"),
        "radios are listed through the group"
    );
    for kind in AgentFormKind::all() {
        let def = kind.definition();
        for entry in def.boxes.iter().filter(|entry| entry.radio_group) {
            assert_eq!(entry.options.len(), entry.members.len(), "{}", entry.key);
            assert!(entry.options.len() >= 2, "{}", entry.key);
            for member in &entry.members {
                assert_eq!(
                    def.box_def(member).and_then(|radio| radio.group.as_deref()),
                    Some(entry.key.as_str())
                );
            }
        }
    }
}

/// The page's profile edit writes every open profile box with a value of
/// its own type, and still refuses every identity box (TIN, year, period).
#[test]
fn the_page_writes_profile_boxes_and_never_identity_boxes() {
    use agent::{DatePart, Lock};
    let (mut written, mut periods) = (0, 0);
    for kind in AgentFormKind::all() {
        let def = kind.definition();
        let seed = seeds_for(kind).remove(0);
        for (key, bound) in &def.bindings {
            let Some(lock @ (Lock::Profile | Lock::Identity)) = bound.lock else {
                continue;
            };
            let entry = def.box_def(key).expect("a bound key is a box");
            if entry.computed || entry.required == "computed" || !entry.is_user_box() {
                continue;
            }
            let value = match &bound.binding {
                Binding::Text {
                    part: Some(DatePart::Month),
                    ..
                } => Value::from("01"),
                Binding::Text {
                    part: Some(DatePart::Day),
                    ..
                } => Value::from("02"),
                Binding::Text {
                    part: Some(DatePart::Year),
                    ..
                } => Value::from("2020"),
                Binding::Text { date: Some(_), .. } => Value::from("01/02/2020"),
                Binding::Text { .. } => Value::from("7193"),
                Binding::Amount { .. } | Binding::Whole { .. } => Value::from("1"),
                Binding::Choice { options } => Value::from(options[0].value.clone()),
            };
            let mut draft = seed.clone();
            let result = agent::fill_profile_box_as_user(kind, &mut draft, key, &value);
            match lock {
                Lock::Identity => {
                    let refused = result.expect_err(key);
                    assert!(refused.contains(lock.reason()), "{key}: {refused}");
                    // Only the host's period writer lifts it, and only on a
                    // period box: the TIN stays refused.
                    let mut draft = seed.clone();
                    let sample = agent::write_period_box(kind, &mut draft, key, &value);
                    let refused = sample.err().unwrap_or_default();
                    let period = agent::writes_only_the_period(&bound.binding);
                    assert_eq!(
                        refused.contains(lock.reason()),
                        !period,
                        "{key} (period box: {period}): {refused}"
                    );
                    periods += usize::from(period);
                }
                _ => {
                    result.unwrap_or_else(|error| {
                        panic!("{} {key} = {value}: {error}", kind.form_code())
                    });
                    written += 1;
                    // The host's period writer never lifts a profile lock.
                    let mut draft = seed.clone();
                    let refused =
                        agent::write_period_box(kind, &mut draft, key, &value).expect_err(key);
                    assert!(refused.contains(lock.reason()), "{key}: {refused}");
                }
            }
        }
    }
    assert!(written > 100, "{written} profile boxes written");
    assert!(periods > 40, "{periods} period boxes");
}
