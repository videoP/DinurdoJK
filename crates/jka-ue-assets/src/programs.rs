//! Inspect exported executable dependencies without pretending metadata is
//! executable shader/Blueprint code. Used by any UE asset importer.
use serde_json::{json, Value};
pub fn inventory(package: &Value) -> Value {
    fn tokens(value: &Value, out: &mut std::collections::BTreeSet<String>) {
        match value {
            Value::Object(fields) => {
                if let Some(token) = fields.get("Token").and_then(Value::as_str) {
                    out.insert(token.into());
                }
                for v in fields.values() {
                    tokens(v, out)
                }
            }
            Value::Array(a) => {
                for v in a {
                    tokens(v, out)
                }
            }
            _ => {}
        }
    }
    let mut blueprints = Vec::new();
    let mut niagara = Vec::new();
    let mut materials = Vec::new();
    for object in package.as_array().into_iter().flatten() {
        let properties = &object["Properties"];
        match object["Type"].as_str().unwrap_or("") {
            "Function" => {
                let mut kinds = std::collections::BTreeSet::new();
                tokens(&object["ScriptBytecode"], &mut kinds);
                blueprints.push(json!({"name":object["Name"],"statements":object["ScriptBytecode"].as_array().map_or(0,Vec::len),"tokens":kinds,"native":object["FunctionFlags"].as_str().is_some_and(|s|s.contains("FUNC_Native"))}));
            }
            "NiagaraScript" => {
                let vm = &properties["CachedScriptVM"];
                niagara.push(json!({"name":object["Name"],"usage":properties["Usage"],"bytecode_bytes":vm["ByteCode"]["Data"].as_array().map_or(0,Vec::len),"registers":vm["NumTempRegisters"],"literal_bytes":vm["ScriptLiterals"].as_array().map_or(0,Vec::len),"engine_bindings":vm["CalledVMExternalFunctions"],"gpu_resources_exported":object.get("LoadedScriptResources").is_some()}));
            }
            "Material" => {
                materials.push(json!({"name":object["Name"],"editor_expressions":properties["Expressions"].as_array().map_or(0,Vec::len),"cooked":true}));
            }
            _ => {}
        }
    }
    json!({"blueprint_functions":blueprints,"niagara_scripts":niagara,"materials":materials,"native_parents":package.as_array().into_iter().flatten().filter(|o|o["Type"].as_str().is_some_and(|s|s.ends_with("GeneratedClass"))).map(|o|o["SuperStruct"].clone()).collect::<Vec<_>>()})
}
