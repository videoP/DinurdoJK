//! Relationships come from item links and full asset paths, never ID prefixes.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub fn relationships(items: &[Value]) -> Value {
    let mut by_asset: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut candidates: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    let names: BTreeMap<String, String> = items
        .iter()
        .filter_map(|i| Some((i["id"].as_str()?.to_owned(), i["name"].as_str()?.to_owned())))
        .collect();
    for item in items {
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        for key in ["Animation", "AnimationFemaleOverride"] {
            if let Some(asset) = item["properties"][key]["AssetPathName"]
                .as_str()
                .filter(|a| !a.is_empty() && *a != "None")
            {
                by_asset
                    .entry(asset.to_ascii_lowercase())
                    .or_default()
                    .insert(id.to_owned());
            }
        }
    }
    for ids in by_asset.values() {
        for child in ids {
            for parent in ids.iter().filter(|p| *p != child) {
                candidates
                    .entry(child.clone())
                    .or_default()
                    .entry(parent.clone())
                    .or_default()
                    .insert("shared_montage".into());
            }
        }
    }
    for item in items {
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        for (key, value) in item["properties"].as_object().into_iter().flatten() {
            if key.starts_with("GroupEmoteToStart") {
                let object = value["ObjectName"].as_str().unwrap_or("");
                let child = object.split('\'').nth(1).unwrap_or("");
                if child != id && names.contains_key(child) {
                    candidates
                        .entry(child.into())
                        .or_default()
                        .entry(id.into())
                        .or_default()
                        .insert(format!("item.{key}"));
                }
            }
        }
    }
    // A normal item can point to an unnamed sync leader which in turn points
    // to its follower. Resolve that chain without walking through another
    // independently named emote or guessing an EID prefix.
    let direct = candidates.clone();
    loop {
        let snapshot = candidates.clone();
        let mut changed = false;
        for (child, parents) in &direct {
            for (parent, edge) in parents {
                if !names.get(parent).is_some_and(|n| n.starts_with("EID_")) {
                    continue;
                }
                for (ancestor, reasons) in snapshot.get(parent).into_iter().flat_map(|p| p.iter()) {
                    if ancestor == child {
                        continue;
                    }
                    let dst = candidates
                        .entry(child.clone())
                        .or_default()
                        .entry(ancestor.clone())
                        .or_default();
                    let before = dst.len();
                    dst.extend(edge.iter().cloned());
                    dst.extend(reasons.iter().cloned());
                    changed |= dst.len() != before;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut links = BTreeMap::new();
    for (child, parents) in candidates {
        if names.get(&child).is_none_or(|n| !n.starts_with("EID_")) {
            continue;
        }
        let mut parents: Vec<_> = parents
            .into_iter()
            .filter(|(id, _)| names.get(id).is_some_and(|n| !n.starts_with("EID_")))
            .collect();
        parents.sort_by_key(|(id, _)| {
            (
                id.to_ascii_lowercase().contains("npc")
                    || id.contains("JoinAdHoc")
                    || names[id].starts_with("[PH]"),
                id.clone(),
            )
        });
        let Some((parent, reasons)) = parents.first() else {
            continue;
        };
        // Conflicting ordinary named owners stay visible for investigation.
        if parents
            .iter()
            .filter(|(id, _)| {
                !id.to_ascii_lowercase().contains("npc")
                    && !id.contains("JoinAdHoc")
                    && !names[id].starts_with("[PH]")
            })
            .map(|(id, _)| &names[id])
            .collect::<BTreeSet<_>>()
            .len()
            > 1
        {
            continue;
        }
        let shared_montages: Vec<_> = by_asset
            .iter()
            .filter(|(_, ids)| ids.contains(&child) && ids.contains(parent))
            .map(|(path, _)| path.rsplit('.').next().unwrap_or("").to_owned())
            .collect();
        links.insert(child.clone(),json!({"parent":parent,"title":names[parent],"reasons":reasons,"shared_montages":shared_montages}));
    }
    json!({"version":1,"links":links})
}

pub fn index_source(source: &Path) -> Result<Value, String> {
    let mut items = Vec::new();
    let mut unavailable = Vec::new();
    for dir in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let path = dir.map_err(|e| e.to_string())?.path();
        if !path.join("ExportReport.json").is_file() || !path.join("Item.json").is_file() {
            continue;
        }
        let load = |p: &Path| -> Result<Value, String> {
            let b = std::fs::read(p).map_err(|e| e.to_string())?;
            serde_json::from_slice(b.strip_prefix(&[239, 187, 191]).unwrap_or(&b))
                .map_err(|e| e.to_string())
        };
        let report = load(&path.join("ExportReport.json"))?;
        let item = load(&path.join("Item.json"))?;
        if report["AnimationCount"].as_u64().unwrap_or(0) == 0 {
            unavailable
                .push(json!({"id":report["Id"],"title":report["Name"],"errors":report["Errors"]}));
        }
        items
            .push(json!({"id":report["Id"],"name":report["Name"],"properties":item["Properties"]}));
    }
    let mut result = relationships(&items);
    unavailable.sort_by_key(|v| v["id"].as_str().unwrap_or("").to_owned());
    result["unavailable"] = json!(unavailable);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follower_of_an_internal_sync_leader_resolves_to_its_named_owner() {
        let rows = vec![
            json!({"id":"EID_Dance","name":"Dance","properties":{"Animation":{"AssetPathName":"/dance.Main"}}}),
            json!({"id":"EID_Sync","name":"EID_Sync","properties":{"Animation":{"AssetPathName":"/dance.Main"},"GroupEmoteToStartFollower":{"ObjectName":"AthenaDanceItemDefinition'EID_Follower'"}}}),
            json!({"id":"EID_Follower","name":"EID_Follower","properties":{"Animation":{"AssetPathName":"/dance.Follower"}}}),
        ];
        let links = relationships(&rows);
        assert_eq!(links["links"]["EID_Follower"]["parent"], "EID_Dance");
        assert_eq!(links["links"]["EID_Follower"]["shared_montages"], json!([]));
    }
    #[test]
    fn shared_assets_and_explicit_followers_get_titles_but_similar_ids_do_not() {
        let rows = vec![
            json!({"id":"EID_Dance","name":"Dance","properties":{"Animation":{"AssetPathName":"/dance.Main"},"GroupEmoteToStartFollower":{"ObjectName":"AthenaDanceItemDefinition'EID_Internal'"}}}),
            json!({"id":"EID_Copy","name":"EID_Copy","properties":{"Animation":{"AssetPathName":"/DANCE.Main"}}}),
            json!({"id":"EID_Internal","name":"EID_Internal","properties":{"Animation":{"AssetPathName":"/dance.Follower"}}}),
            json!({"id":"EID_Dance_Intro","name":"EID_Dance_Intro","properties":{"Animation":{"AssetPathName":"/intro.Separate"}}}),
        ];
        let links = relationships(&rows);
        assert_eq!(links["links"]["EID_Copy"]["parent"], "EID_Dance");
        assert_eq!(links["links"]["EID_Internal"]["title"], "Dance");
        assert!(links["links"]["EID_Dance_Intro"].is_null());
    }
}
