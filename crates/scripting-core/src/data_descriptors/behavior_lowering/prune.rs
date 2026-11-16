// The exhaustive `attacks` list's drops: rows to unlisted attacks, then the
// authored rows of every rest no longer reached, repeated to a fixed point
// before stance expansion checks what is left.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::declarations::Catalog;
use super::{expand, for_each_nested_envelope, is_handle_of};

/// Drop what an explicit `attacks` list excludes, repeating until nothing
/// more drops: every row to an unlisted attack, then the authored rows of any
/// rest no longer reached. Each dropped row logs once, at its authored index,
/// and its guard is never checked, so a variant reuses its parent's graph with
/// a shorter list.
pub(super) fn prune_to_fixed_point(
    root: &mut Map<String, Value>,
    path: &str,
    listed: &BTreeSet<String>,
    catalog: &Catalog,
    archetype: Option<&str>,
) {
    loop {
        let dropped = drop_rows_to_unlisted_attacks(root, path, listed, archetype)
            + drop_unreached_rest_rows(root, path, catalog, archetype);
        if dropped == 0 {
            break;
        }
    }
    strip_dropped_rows(root, path);
}

/// Drop every row, in this envelope and those nested in it, whose `to` is an
/// attack the list leaves out. A dropped row stays as a `null` placeholder
/// until the fixed point, so later drops log their authored indices. Returns
/// how many rows dropped.
fn drop_rows_to_unlisted_attacks(
    envelope: &mut Map<String, Value>,
    path: &str,
    listed: &BTreeSet<String>,
    archetype: Option<&str>,
) -> usize {
    let mut dropped = 0;
    if let Some(transitions) = envelope
        .get_mut("transitions")
        .and_then(Value::as_object_mut)
    {
        for (source, rows) in transitions.iter_mut() {
            for (index, row) in rows.as_array_mut().into_iter().flatten().enumerate() {
                let Some(name) = row
                    .get("to")
                    .filter(|target| is_handle_of(target, "attack"))
                    .and_then(|target| target.get("name"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if listed.contains(name) {
                    continue;
                }
                log::info!(
                    "[Scripting] {}: `{path}.transitions.{source}[{index}]` drops its row to attack \"{name}\", which the behavior's `attacks` list leaves out",
                    archetype.unwrap_or("<unnamed archetype>"),
                );
                *row = Value::Null;
                dropped += 1;
            }
        }
    }
    for_each_nested_envelope(envelope, path, |nested, nested_path| {
        dropped += drop_rows_to_unlisted_attacks(nested, nested_path, listed, archetype);
    });
    dropped
}

/// Drop the authored rows of every stance rest this envelope, or one nested
/// in it, no longer reaches. Returns how many rows dropped.
fn drop_unreached_rest_rows(
    envelope: &mut Map<String, Value>,
    path: &str,
    catalog: &Catalog,
    archetype: Option<&str>,
) -> usize {
    let reach = expand::reach(envelope, path, catalog);
    let authored = expand::authored_activities(envelope);
    let mut dropped = 0;
    if let Some(transitions) = envelope
        .get_mut("transitions")
        .and_then(Value::as_object_mut)
    {
        let unreached: Vec<String> = transitions
            .keys()
            .filter(|source| {
                expand::is_rest_key(source, &authored, catalog) && !reach.rests.contains(*source)
            })
            .cloned()
            .collect();
        for stance in unreached {
            let Some(Value::Array(rows)) = transitions.remove(&stance) else {
                continue;
            };
            for (index, row) in rows.iter().enumerate() {
                if row.is_null() {
                    continue;
                }
                log::info!(
                    "[Scripting] {}: `{path}.transitions.{stance}[{index}]` drops with stance \"{stance}\"'s rest, which nothing reaches once the behavior's `attacks` list drops its rows",
                    archetype.unwrap_or("<unnamed archetype>"),
                );
                dropped += 1;
            }
        }
    }
    for_each_nested_envelope(envelope, path, |nested, nested_path| {
        dropped += drop_unreached_rest_rows(nested, nested_path, catalog, archetype);
    });
    dropped
}

/// Remove the placeholders dropped rows left, here and in nested envelopes.
fn strip_dropped_rows(envelope: &mut Map<String, Value>, path: &str) {
    if let Some(transitions) = envelope
        .get_mut("transitions")
        .and_then(Value::as_object_mut)
    {
        for rows in transitions.values_mut() {
            if let Some(rows) = rows.as_array_mut() {
                rows.retain(|row| !row.is_null());
            }
        }
    }
    for_each_nested_envelope(envelope, path, strip_dropped_rows);
}
