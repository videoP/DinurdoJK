//! Inventory of the executable program data (Blueprint bytecode, Niagara
//! VectorVM kernels, material resources) in an export, and whether each piece
//! decodes with `jka-ue-runtime`. Decoding is not execution: every entry notes
//! the host bindings it would still need.
use crate::compiler::read;
use crate::MAX_MEMBER_BYTES;
use serde_json::{json, Value};
use std::{fs, path::Path};

/// Write `output` (JSON) describing every program-carrying package in the export at `directory`.
pub fn run(directory: &Path, output: &Path) -> Result<(), String> {
    let directory = directory;
    let report: Value =
        serde_json::from_slice(&read(&directory.join("report.json"), MAX_MEMBER_BYTES)?)
            .map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    let mut packages = std::collections::BTreeSet::new();
    for row in report["assets"]
        .as_array()
        .ok_or("export report missing assets")?
    {
        if !row["type"]
            .as_str()
            .is_some_and(|t| t.contains("Niagara") || t.contains("Blueprint") || t == "Material")
        {
            continue;
        }
        let package_name = row["asset"]
            .as_str()
            .unwrap_or("")
            .split('.')
            .next()
            .unwrap_or("");
        if !packages.insert(package_name.to_owned()) {
            continue;
        }
        let Some(id) = row["id"]
            .as_str()
            .filter(|n| n.len() == 24 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        else {
            continue;
        };
        let path = directory.join("Objects").join(format!("{id}.json"));
        if !path.is_file() {
            continue;
        }
        let package: Value =
            serde_json::from_slice(&read(&path, MAX_MEMBER_BYTES)?).map_err(|e| e.to_string())?;
        let mut inventory = jka_ue_assets::programs::inventory(&package);
        let mut kernels = Vec::new();
        let mut blueprints = Vec::new();
        for object in package
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["Type"] == "Function")
        {
            let result = jka_ue_runtime::blueprint::Function::parse(object);
            blueprints.push(json!({"name":object["Name"],"instructions_supported":result.is_ok(),"diagnostic":result.err(),"object_and_native_host_bindings_required":true}));
        }
        for object in package
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["Type"] == "NiagaraScript")
        {
            let vm = &object["Properties"]["CachedScriptVM"];
            let bytes = vm["ByteCode"]["Data"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|b| {
                    b.as_u64()
                        .filter(|n| *n < 256)
                        .map(|n| n as u8)
                        .ok_or("invalid Niagara bytecode byte")
                })
                .collect::<Result<Vec<_>, _>>()?;
            if bytes.is_empty() {
                continue;
            }
            let functions = vm["CalledVMExternalFunctions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| jka_ue_runtime::vector_vm::External {
                    owner: f["OwnerName"].as_str().unwrap_or("").into(),
                    name: f["Name"].as_str().unwrap_or("").into(),
                    inputs: f["InputParamLocations"].as_array().map_or(0, Vec::len),
                    outputs: f["NumOutputs"].as_u64().unwrap_or(0) as usize,
                })
                .collect();
            let result = jka_ue_runtime::vector_vm::Kernel::decode(
                &bytes,
                vm["NumTempRegisters"].as_u64().unwrap_or(0) as usize,
                functions,
            );
            kernels.push(json!({"name":object["Name"],"bytecode_decoded":result.is_ok(),"diagnostic":result.err(),"host_dataset_and_native_bindings_required":true}));
        }
        let program_path = directory.join("Programs").join(format!("{id}.json"));
        if program_path.is_file() {
            let programs: Value = serde_json::from_slice(&read(&program_path, MAX_MEMBER_BYTES)?)
                .map_err(|e| e.to_string())?;
            inventory["native_program_resources"]=json!(programs.as_array().into_iter().flatten().map(|p|json!({"type":p["Type"],"name":p["Name"],"fields":p["NativeData"].as_object().map(|o|o.iter().filter(|(_,v)|!v.is_null()&&!v.as_array().is_some_and(Vec::is_empty)).map(|(k,_)|k.clone()).collect::<Vec<_>>()).unwrap_or_default()})).collect::<Vec<_>>());
        }
        inventory["blueprint_decode"] = json!(blueprints);
        inventory["kernel_decode"] = json!(kernels);
        inventory["asset"] = row["asset"].clone();
        records.push(inventory);
    }
    fs::write(
        output,
        serde_json::to_vec_pretty(
            &json!({"version":1,"assets":records,"full_unreal_runtime":false}),
        )
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!("Audited {} executable dependency packages", records.len());
    Ok(())
}
