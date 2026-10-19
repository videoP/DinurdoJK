//! Bounded execution of exported Kismet expressions. Object lifetime, native
//! functions, reflection and latent actions belong to the embedding host.
//! This accepts CUE4Parse's instruction AST, not optimized native machine code.
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub trait Host {
    fn read_property(&self, object: &Value, name: &str) -> Result<Value, String>;
    fn write_property(&mut self, object: &Value, name: &str, value: Value) -> Result<(), String>;
    /// In/out arguments preserve Blueprint reference parameters. The host must
    /// validate function signatures and object handles before executing them.
    fn call(
        &mut self,
        object: &Value,
        function: &Value,
        args: &mut [Value],
    ) -> Result<Value, String>;
    fn is_valid(&self, object: &Value) -> bool {
        !object.is_null()
    }
    /// Class inheritance/interface checks must use the host's imported class
    /// registry, including native parents. Do not guess from object names.
    fn cast_object(&self, _kind: &str, _object: &Value, _class: &Value) -> Result<Value, String> {
        Err("Kismet class/interface registry is not bound".into())
    }
    fn interface_object(&self, _interface: &Value) -> Result<Value, String> {
        Err("Kismet interface binding is not available".into())
    }
}
#[derive(Default, Debug)]
pub struct Frame {
    pub this: Value,
    pub locals: BTreeMap<String, Value>,
    pub persistent: BTreeMap<String, Value>,
}
#[derive(Debug)]
pub struct Function {
    pub name: String,
    statements: Vec<Value>,
    addresses: BTreeMap<u64, usize>,
}
#[derive(Debug, Clone)]
enum Location {
    Local(String),
    Persistent(String),
    Property(Value, String),
    Element(Box<Location>, usize),
    Member(Box<Location>, String),
}
struct Execution<'a, H> {
    frame: &'a mut Frame,
    host: &'a mut H,
    remaining: usize,
    frame_object: Value,
}
fn token(v: &Value) -> Result<&str, String> {
    v["Token"].as_str().ok_or("missing Kismet token".into())
}
fn property(v: &Value) -> Result<String, String> {
    v["Property"]["Name"]
        .as_str()
        .or_else(|| v["Name"].as_str())
        .filter(|n| !n.is_empty() && n.len() <= 1024)
        .map(str::to_owned)
        .ok_or("Kismet property pointer has no field name".into())
}
fn truth(v: &Value) -> Result<bool, String> {
    v.as_bool()
        .or_else(|| v.as_i64().map(|n| n != 0))
        .ok_or("Kismet predicate is not a boolean".into())
}
impl Function {
    pub fn parse(object: &Value) -> Result<Self, String> {
        if object["Type"] != "Function" {
            return Err("not a Kismet Function export".into());
        }
        let statements = object["ScriptBytecode"]
            .as_array()
            .filter(|n| !n.is_empty() && n.len() <= 100_000)
            .ok_or("missing or oversized Kismet bytecode")?;
        if serde_json::to_vec(object).map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
            return Err("Kismet function exceeds byte limit".into());
        }
        let mut addresses = BTreeMap::new();
        for (i, v) in statements.iter().enumerate() {
            let address = v["StatementIndex"]
                .as_u64()
                .ok_or("missing Kismet statement address")?;
            if addresses.insert(address, i).is_some() {
                return Err("duplicate Kismet statement address".into());
            }
            Self::validate(v, 0)?;
        }
        if token(statements.last().unwrap())? != "EX_EndOfScript" {
            return Err("Kismet function has no EndOfScript".into());
        }
        Ok(Self {
            name: object["Name"].as_str().unwrap_or("Function").into(),
            statements: statements.clone(),
            addresses,
        })
    }
    fn validate(v: &Value, depth: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("Kismet expression nesting exceeds limit".into());
        }
        match v {
            Value::Object(map) => {
                if let Some(t) = map.get("Token").and_then(Value::as_str) {
                    match t {
                        "EX_Cast"
                        | "EX_DynamicCast"
                        | "EX_MetaCast"
                        | "EX_ObjToInterfaceCast"
                        | "EX_InterfaceToObjCast"
                        | "EX_InterfaceContext"
                        | "EX_BindDelegate"
                        | "EX_AddMulticastDelegate"
                        | "EX_RemoveMulticastDelegate"
                        | "EX_ClearMulticastDelegate"
                        | "EX_CallMulticastDelegate"
                        | "EX_LetDelegate"
                        | "EX_LetMulticastDelegate"
                        | "EX_SetArray"
                        | "EX_Nothing"
                        | "EX_EndOfScript"
                        | "EX_Return"
                        | "EX_LocalVariable"
                        | "EX_LocalOutVariable"
                        | "EX_InstanceVariable"
                        | "EX_IntZero"
                        | "EX_IntOne"
                        | "EX_True"
                        | "EX_False"
                        | "EX_Self"
                        | "EX_NoObject"
                        | "EX_NoInterface"
                        | "EX_IntConst"
                        | "EX_IntConstByte"
                        | "EX_Int64Const"
                        | "EX_UInt64Const"
                        | "EX_ByteConst"
                        | "EX_FloatConst"
                        | "EX_DoubleConst"
                        | "EX_StringConst"
                        | "EX_UnicodeStringConst"
                        | "EX_NameConst"
                        | "EX_ObjectConst"
                        | "EX_VectorConst"
                        | "EX_Vector3fConst"
                        | "EX_RotationConst"
                        | "EX_TransformConst"
                        | "EX_SkipOffsetConst"
                        | "EX_Let"
                        | "EX_LetBool"
                        | "EX_LetObj"
                        | "EX_LetWeakObjPtr"
                        | "EX_LetValueOnPersistentFrame"
                        | "EX_StructConst"
                        | "EX_ArrayConst"
                        | "EX_SetConst"
                        | "EX_MapConst"
                        | "EX_ArrayGetByRef"
                        | "EX_StructMemberContext"
                        | "EX_Context"
                        | "EX_Context_FailSilent"
                        | "EX_ClassContext"
                        | "EX_FinalFunction"
                        | "EX_LocalFinalFunction"
                        | "EX_CallMath"
                        | "EX_VirtualFunction"
                        | "EX_LocalVirtualFunction"
                        | "EX_SwitchValue"
                        | "EX_Jump"
                        | "EX_JumpIfNot"
                        | "EX_ComputedJump"
                        | "EX_PushExecutionFlow"
                        | "EX_PopExecutionFlow"
                        | "EX_PopExecutionFlowIfNot" => {}
                        _ => return Err(format!("unported Kismet instruction {t}")),
                    }
                }
                for child in map.values() {
                    Self::validate(child, depth + 1)?;
                }
            }
            Value::Array(a) => {
                for child in a {
                    Self::validate(child, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn execute(
        &self,
        frame: &mut Frame,
        host: &mut impl Host,
        budget: usize,
    ) -> Result<Value, String> {
        if budget == 0 || budget > 1_000_000 {
            return Err("invalid Kismet execution budget".into());
        }
        let frame_object = frame.this.clone();
        let mut vm = Execution {
            frame,
            host,
            remaining: budget,
            frame_object,
        };
        let mut pc = 0;
        let mut flow = Vec::new();
        let destination = |address: u64| {
            self.addresses
                .get(&address)
                .copied()
                .ok_or_else(|| format!("Kismet branch to non-statement address {address}"))
        };
        while let Some(statement) = self.statements.get(pc) {
            vm.step()?;
            let mut jump = None;
            match token(statement)? {
                "EX_EndOfScript" => return Ok(Value::Null),
                "EX_Return" => return vm.eval(&statement["Expression"], 0),
                "EX_Jump" => {
                    jump = Some(
                        statement["CodeOffset"]
                            .as_u64()
                            .ok_or("invalid Kismet jump")?,
                    )
                }
                "EX_JumpIfNot" => {
                    if !truth(&vm.eval(&statement["BooleanExpression"], 0)?)? {
                        jump = Some(
                            statement["CodeOffset"]
                                .as_u64()
                                .ok_or("invalid Kismet jump")?,
                        );
                    }
                }
                "EX_ComputedJump" => {
                    jump = Some(
                        vm.eval(&statement["CodeOffsetExpression"], 0)?
                            .as_u64()
                            .ok_or("invalid computed Kismet jump")?,
                    )
                }
                "EX_PushExecutionFlow" => {
                    if flow.len() >= 1024 {
                        return Err("Kismet flow stack overflow".into());
                    }
                    let target = statement["PushingAddress"]
                        .as_u64()
                        .ok_or("invalid Kismet flow address")?;
                    destination(target)?;
                    flow.push(target);
                }
                "EX_PopExecutionFlow" => {
                    jump = Some(flow.pop().ok_or("Kismet flow stack underflow")?)
                }
                "EX_PopExecutionFlowIfNot" => {
                    if !truth(&vm.eval(&statement["BooleanExpression"], 0)?)? {
                        jump = Some(flow.pop().ok_or("Kismet flow stack underflow")?);
                    }
                }
                _ => {
                    vm.eval(statement, 0)?;
                }
            }
            pc = if let Some(address) = jump {
                destination(address)?
            } else {
                pc + 1
            };
        }
        Err("Kismet ran outside its function".into())
    }
}
impl<H: Host> Execution<'_, H> {
    fn parameters(
        &mut self,
        parameters: &Value,
        depth: usize,
    ) -> Result<(Vec<Value>, Vec<Option<Location>>), String> {
        // UE evaluates call arguments with Stack.Object, even when the target
        // function runs inside EX_Context on another object.
        let previous = std::mem::replace(&mut self.frame.this, self.frame_object.clone());
        let result = (|| {
            let mut args = Vec::new();
            let mut destinations = Vec::new();
            for parameter in parameters
                .as_array()
                .ok_or("Kismet function parameters missing")?
            {
                let location = if matches!(
                    token(parameter)?,
                    "EX_LocalVariable"
                        | "EX_LocalOutVariable"
                        | "EX_InstanceVariable"
                        | "EX_ArrayGetByRef"
                ) {
                    Some(self.location(parameter, depth + 1)?)
                } else {
                    None
                };
                args.push(if let Some(l) = &location {
                    self.read(l)?
                } else {
                    self.eval(parameter, depth + 1)?
                });
                destinations.push(location);
            }
            Ok((args, destinations))
        })();
        self.frame.this = previous;
        result
    }
    fn outputs(
        &mut self,
        destinations: &[Option<Location>],
        args: Vec<Value>,
        originals: Vec<Value>,
    ) -> Result<(), String> {
        for ((location, value), old) in destinations.iter().zip(args).zip(originals) {
            if value != old {
                self.write(
                    location
                        .as_ref()
                        .ok_or("native function changed a non-reference parameter")?,
                    value,
                )?;
            }
        }
        Ok(())
    }
    fn step(&mut self) -> Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or("Kismet execution budget exhausted")?;
        Ok(())
    }
    fn location(&mut self, v: &Value, depth: usize) -> Result<Location, String> {
        if depth > 64 {
            return Err("Kismet reference nesting exceeds limit".into());
        }
        Ok(match token(v)? {
            "EX_LocalVariable" | "EX_LocalOutVariable" => {
                Location::Local(property(&v["Variable"])?)
            }
            "EX_InstanceVariable" => {
                Location::Property(self.frame.this.clone(), property(&v["Variable"])?)
            }
            "EX_ArrayGetByRef" => {
                let location = self.location(&v["ArrayVariable"], depth + 1)?;
                let index = self
                    .eval(&v["ArrayIndex"], depth + 1)?
                    .as_u64()
                    .filter(|n| *n < 65536)
                    .ok_or("invalid Kismet array index")? as usize;
                Location::Element(Box::new(location), index)
            }
            "EX_StructMemberContext" => Location::Member(
                Box::new(self.location(&v["StructExpression"], depth + 1)?),
                property(&v["Property"])?,
            ),
            _ => return Err("Kismet assignment requires a property reference".into()),
        })
    }
    fn read(&self, l: &Location) -> Result<Value, String> {
        match l {
            Location::Local(n) => self
                .frame
                .locals
                .get(n)
                .cloned()
                .ok_or_else(|| format!("uninitialized Kismet local {n}")),
            Location::Persistent(n) => self
                .frame
                .persistent
                .get(n)
                .cloned()
                .ok_or_else(|| format!("uninitialized Kismet persistent field {n}")),
            Location::Property(o, n) => self.host.read_property(o, n),
            Location::Element(l, i) => self
                .read(l)?
                .as_array()
                .and_then(|a| a.get(*i))
                .cloned()
                .ok_or("Kismet array index outside value".into()),
            Location::Member(l, n) => self
                .read(l)?
                .as_object()
                .and_then(|o| o.get(n))
                .cloned()
                .ok_or_else(|| format!("unbound Kismet struct member {n}")),
        }
    }
    fn write(&mut self, l: &Location, value: Value) -> Result<(), String> {
        match l {
            Location::Local(n) => {
                self.frame.locals.insert(n.clone(), value);
                Ok(())
            }
            Location::Persistent(n) => {
                self.frame.persistent.insert(n.clone(), value);
                Ok(())
            }
            Location::Property(o, n) => self.host.write_property(o, n, value),
            Location::Element(parent, index) => {
                let mut array = self.read(parent)?;
                *array
                    .as_array_mut()
                    .and_then(|a| a.get_mut(*index))
                    .ok_or("Kismet array index outside value")? = value;
                self.write(parent, array)
            }
            Location::Member(parent, name) => {
                let mut object = self.read(parent)?;
                *object
                    .as_object_mut()
                    .and_then(|o| o.get_mut(name))
                    .ok_or("unbound Kismet struct member")? = value;
                self.write(parent, object)
            }
        }
    }
    fn eval(&mut self, v: &Value, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err("Kismet expression nesting exceeds limit".into());
        }
        self.step()?;
        let result = match token(v)? {
            "EX_Nothing" | "EX_NoObject" | "EX_NoInterface" => Value::Null,
            "EX_IntZero" => json!(0),
            "EX_IntOne" => json!(1),
            "EX_True" => json!(true),
            "EX_False" => json!(false),
            "EX_Self" => self.frame.this.clone(),
            "EX_LocalVariable"
            | "EX_LocalOutVariable"
            | "EX_InstanceVariable"
            | "EX_ArrayGetByRef" => {
                let l = self.location(v, depth + 1)?;
                self.read(&l)?
            }
            "EX_StructMemberContext" => {
                let object = self.eval(&v["StructExpression"], depth + 1)?;
                object
                    .get(property(&v["Property"])?)
                    .cloned()
                    .ok_or("unbound Kismet struct member")?
            }
            "EX_IntConst"
            | "EX_IntConstByte"
            | "EX_Int64Const"
            | "EX_UInt64Const"
            | "EX_ByteConst"
            | "EX_FloatConst"
            | "EX_DoubleConst"
            | "EX_StringConst"
            | "EX_UnicodeStringConst"
            | "EX_NameConst"
            | "EX_ObjectConst"
            | "EX_VectorConst"
            | "EX_Vector3fConst"
            | "EX_RotationConst"
            | "EX_TransformConst"
            | "EX_SkipOffsetConst" => v
                .get("Value")
                .ok_or("Kismet constant has no value")?
                .clone(),
            "EX_Let"
            | "EX_LetBool"
            | "EX_LetObj"
            | "EX_LetWeakObjPtr"
            | "EX_LetDelegate"
            | "EX_LetMulticastDelegate" => {
                let dest = self.location(&v["Variable"], depth + 1)?;
                let value = self.eval(&v["Expression"], depth + 1)?;
                self.write(&dest, value.clone())?;
                value
            }
            "EX_LetValueOnPersistentFrame" => {
                let dest = Location::Persistent(property(&v["DestinationProperty"])?);
                let value = self.eval(&v["AssignmentExpression"], depth + 1)?;
                self.write(&dest, value.clone())?;
                value
            }
            "EX_StructConst" => {
                let fields = v["Properties"]
                    .as_array()
                    .ok_or("Kismet struct properties missing")?
                    .iter()
                    .map(|p| self.eval(p, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                // Positional fields stay positional until a host supplies the
                // UScriptStruct layout. Never infer names from asset text.
                json!({"$struct":v["Struct"],"fields":fields})
            }
            "EX_ArrayConst" | "EX_SetConst" => {
                let mut values = v["Values"]
                    .as_array()
                    .ok_or("Kismet collection values missing")?
                    .iter()
                    .map(|p| self.eval(p, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                if token(v)? == "EX_SetConst" {
                    let mut unique = Vec::new();
                    for value in values {
                        if !unique.contains(&value) {
                            unique.push(value);
                        }
                    }
                    values = unique;
                }
                Value::Array(values)
            }
            "EX_SetArray" => {
                let dest = self.location(&v["AssigningProperty"], depth + 1)?;
                let values = v["Elements"]
                    .as_array()
                    .ok_or("Kismet array elements missing")?
                    .iter()
                    .map(|p| self.eval(p, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = Value::Array(values);
                self.write(&dest, value.clone())?;
                value
            }
            "EX_MapConst" => {
                let mut entries = Vec::new();
                for p in v["Values"].as_array().ok_or("Kismet map values missing")? {
                    entries.push(json!({"Key":self.eval(&p["Key"],depth+1)?,"Value":self.eval(&p["Value"],depth+1)?}));
                }
                json!({"$map":entries})
            }
            "EX_Context" | "EX_Context_FailSilent" | "EX_ClassContext" => {
                let object = self.eval(&v["ObjectExpression"], depth + 1)?;
                if !self.host.is_valid(&object) {
                    if token(v)? == "EX_Context_FailSilent" {
                        Value::Null
                    } else {
                        return Err("Kismet access through a null object".into());
                    }
                } else {
                    let previous = std::mem::replace(&mut self.frame.this, object);
                    let value = self.eval(&v["ContextExpression"], depth + 1);
                    self.frame.this = previous;
                    value?
                }
            }
            "EX_FinalFunction"
            | "EX_LocalFinalFunction"
            | "EX_CallMath"
            | "EX_VirtualFunction"
            | "EX_LocalVirtualFunction" => {
                let (mut args, destinations) = self.parameters(&v["Parameters"], depth + 1)?;
                let originals = args.clone();
                let result = self
                    .host
                    .call(&self.frame.this, &v["Function"], &mut args)?;
                self.outputs(&destinations, args, originals)?;
                result
            }
            "EX_Cast" => {
                let target = self.eval(&v["Target"], depth + 1)?;
                match v["ConversionType"].as_str().unwrap_or("") {
                    "CST_ObjectToBool"
                    | "CST_ObjectToBool2"
                    | "CST_InterfaceToBool"
                    | "CST_InterfaceToBool2" => json!(self.host.is_valid(&target)),
                    "CST_DoubleToFloat" => {
                        let n = target.as_f64().ok_or("Kismet cast requires double")? as f32;
                        if !n.is_finite() {
                            return Err("Kismet float conversion overflows".into());
                        }
                        json!(n)
                    }
                    "CST_FloatToDouble" => {
                        json!((target.as_f64().ok_or("Kismet cast requires float")? as f32) as f64)
                    }
                    kind => return Err(format!("unported Kismet primitive cast {kind}")),
                }
            }
            "EX_DynamicCast"
            | "EX_MetaCast"
            | "EX_ObjToInterfaceCast"
            | "EX_InterfaceToObjCast" => {
                let target = self.eval(&v["Target"], depth + 1)?;
                self.host
                    .cast_object(token(v)?, &target, &v["InterfaceClass"])?
            }
            "EX_InterfaceContext" => {
                let interface = self.eval(&v["InterfaceValue"], depth + 1)?;
                self.host.interface_object(&interface)?
            }
            "EX_BindDelegate" => {
                let dest = self.location(&v["Delegate"], depth + 1)?;
                let object = self.eval(&v["ObjectTerm"], depth + 1)?;
                let delegate = if self.host.is_valid(&object) {
                    json!({"$delegate":{"object":object,"function":v["FunctionName"]}})
                } else {
                    Value::Null
                };
                self.write(&dest, delegate.clone())?;
                delegate
            }
            "EX_AddMulticastDelegate" | "EX_RemoveMulticastDelegate" => {
                let dest = self.location(&v["MulticastDelegate"], depth + 1)?;
                let current = self.read(&dest)?;
                let mut delegates = if current.is_null() {
                    Vec::new()
                } else {
                    current
                        .as_array()
                        .ok_or("invalid Kismet multicast delegate")?
                        .clone()
                };
                let delegate = self.eval(&v["Delegate"], depth + 1)?;
                if token(v)? == "EX_RemoveMulticastDelegate" {
                    delegates.retain(|d| d != &delegate);
                } else if !delegate.is_null() && !delegates.contains(&delegate) {
                    if delegates.len() >= 1024 {
                        return Err("Kismet delegate budget exceeded".into());
                    }
                    delegates.push(delegate);
                }
                let result = Value::Array(delegates);
                self.write(&dest, result.clone())?;
                result
            }
            "EX_ClearMulticastDelegate" => {
                let dest = self.location(&v["DelegateToClear"], depth + 1)?;
                self.write(&dest, json!([]))?;
                Value::Null
            }
            "EX_CallMulticastDelegate" => {
                let delegates = self.eval(&v["Delegate"], depth + 1)?;
                let (mut args, destinations) = self.parameters(&v["Parameters"], depth + 1)?;
                let originals = args.clone();
                for delegate in delegates
                    .as_array()
                    .ok_or("invalid Kismet multicast delegate")?
                {
                    self.step()?;
                    let binding = &delegate["$delegate"];
                    if binding.get("object").is_none() || binding.get("function").is_none() {
                        return Err("invalid Kismet delegate binding".into());
                    }
                    if self.host.is_valid(&binding["object"]) {
                        self.host
                            .call(&binding["object"], &binding["function"], &mut args)?;
                    }
                }
                self.outputs(&destinations, args, originals)?;
                Value::Null
            }
            "EX_SwitchValue" => {
                let index = self.eval(&v["IndexTerm"], depth + 1)?;
                let mut selected = &v["DefaultTerm"];
                for case in v["Cases"].as_array().ok_or("Kismet switch cases missing")? {
                    if self.eval(&case["CaseIndexValueTerm"], depth + 1)? == index {
                        selected = &case["CaseTerm"];
                        break;
                    }
                }
                self.eval(selected, depth + 1)?
            }
            other => return Err(format!("Kismet instruction {other} is not an expression")),
        };
        if serde_json::to_vec(&result)
            .map_err(|e| e.to_string())?
            .len()
            > 1024 * 1024
        {
            return Err("Kismet value exceeds byte limit".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestHost {
        properties: BTreeMap<String, Value>,
        calls: usize,
    }
    impl Host for TestHost {
        fn read_property(&self, _: &Value, n: &str) -> Result<Value, String> {
            self.properties
                .get(n)
                .cloned()
                .ok_or("missing object property".into())
        }
        fn write_property(&mut self, _: &Value, n: &str, v: Value) -> Result<(), String> {
            self.properties.insert(n.into(), v);
            Ok(())
        }
        fn call(&mut self, _: &Value, f: &Value, args: &mut [Value]) -> Result<Value, String> {
            self.calls += 1;
            match f.as_str() {
                Some("Increment") => {
                    args[0] = json!(args[0].as_i64().ok_or("bad int")? + 1);
                    Ok(Value::Null)
                }
                Some("Echo") => Ok(args[0].clone()),
                _ => Err("native function has no binding".into()),
            }
        }
    }
    fn host() -> TestHost {
        TestHost {
            properties: BTreeMap::new(),
            calls: 0,
        }
    }
    fn local(n: &str) -> Value {
        json!({"Token":"EX_LocalVariable","Variable":{"Property":{"Name":n}}})
    }
    fn function(mut statements: Vec<Value>) -> Function {
        statements.push(json!({"Token":"EX_EndOfScript"}));
        for (i, s) in statements.iter_mut().enumerate() {
            s["StatementIndex"] = json!(i * 10);
        }
        Function::parse(&json!({"Type":"Function","Name":"Test","ScriptBytecode":statements}))
            .unwrap()
    }
    #[test]
    fn branches_and_native_reference_parameters_follow_original_statement_addresses() {
        let f = function(vec![
            json!({"Token":"EX_JumpIfNot","CodeOffset":20,"BooleanExpression":local("do_it")}),
            json!({"Token":"EX_FinalFunction","Function":"Increment","Parameters":[local("n")]}),
            json!({"Token":"EX_Return","Expression":local("n")}),
        ]);
        let mut frame = Frame::default();
        frame.locals.insert("n".into(), json!(4));
        frame.locals.insert("do_it".into(), json!(false));
        let mut h = host();
        assert_eq!(f.execute(&mut frame, &mut h, 100).unwrap(), json!(4));
        assert_eq!(h.calls, 0);
        frame.locals.insert("do_it".into(), json!(true));
        assert_eq!(f.execute(&mut frame, &mut h, 100).unwrap(), json!(5));
        assert_eq!(h.calls, 1);
    }
    #[test]
    fn null_silent_context_never_calls_native_code_and_context_restores_on_failure() {
        let call = json!({"Token":"EX_FinalFunction","Function":"Missing","Parameters":[]});
        let f = function(vec![
            json!({"Token":"EX_Context_FailSilent","ObjectExpression":{"Token":"EX_NoObject"},"ContextExpression":call}),
        ]);
        let mut h = host();
        let mut frame = Frame {
            this: json!("player"),
            ..Default::default()
        };
        f.execute(&mut frame, &mut h, 100).unwrap();
        assert_eq!(h.calls, 0);
        let f = function(vec![
            json!({"Token":"EX_Context","ObjectExpression":{"Token":"EX_StringConst","Value":"prop"},"ContextExpression":call}),
        ]);
        assert!(f
            .execute(&mut frame, &mut h, 100)
            .unwrap_err()
            .contains("no binding"));
        assert_eq!(frame.this, json!("player"));
    }
    #[test]
    fn malformed_branches_infinite_loops_and_unported_instructions_are_errors() {
        let mut frame = Frame::default();
        let mut h = host();
        assert!(function(vec![json!({"Token":"EX_Jump","CodeOffset":1})])
            .execute(&mut frame, &mut h, 100)
            .unwrap_err()
            .contains("non-statement"));
        assert!(function(vec![json!({"Token":"EX_Jump","CodeOffset":0})])
            .execute(&mut frame, &mut h, 100)
            .unwrap_err()
            .contains("budget"));
        assert!(Function::parse(&json!({"Type":"Function","ScriptBytecode":[{"Token":"EX_Unknown","StatementIndex":0},{"Token":"EX_EndOfScript","StatementIndex":5}]})).unwrap_err().contains("unported"));
    }
    #[test]
    fn function_arguments_use_caller_self_inside_another_object_context() {
        let f = function(vec![
            json!({"Token":"EX_Return","Expression":{"Token":"EX_Context","ObjectExpression":{"Token":"EX_StringConst","Value":"prop"},"ContextExpression":{"Token":"EX_FinalFunction","Function":"Echo","Parameters":[{"Token":"EX_Self"}]}}}),
        ]);
        let mut frame = Frame {
            this: json!("player"),
            ..Default::default()
        };
        assert_eq!(
            f.execute(&mut frame, &mut host(), 100).unwrap(),
            json!("player")
        );
        assert_eq!(frame.this, json!("player"));
    }
    #[test]
    fn multicast_bind_remove_and_inout_arguments_are_bounded_and_unique() {
        let bind = json!({"Token":"EX_BindDelegate","Delegate":local("handler"),"ObjectTerm":{"Token":"EX_Self"},"FunctionName":"Increment"});
        let add = json!({"Token":"EX_AddMulticastDelegate","MulticastDelegate":local("event"),"Delegate":local("handler")});
        let call = json!({"Token":"EX_CallMulticastDelegate","Delegate":local("event"),"Parameters":[local("n")]});
        let f = function(vec![
            bind,
            add.clone(),
            add,
            call.clone(),
            json!({"Token":"EX_RemoveMulticastDelegate","MulticastDelegate":local("event"),"Delegate":local("handler")}),
            call,
            json!({"Token":"EX_Return","Expression":local("n")}),
        ]);
        let mut frame = Frame {
            this: json!("player"),
            ..Default::default()
        };
        frame.locals.insert("event".into(), Value::Null);
        frame.locals.insert("n".into(), json!(4));
        let mut h = host();
        assert_eq!(f.execute(&mut frame, &mut h, 200).unwrap(), json!(5));
        assert_eq!(h.calls, 1);
        assert_eq!(frame.locals["event"], json!([]));
    }
    #[test]
    fn primitive_casts_and_exported_animation_graph_assignment_execute() {
        let f = function(vec![
            json!({"Token":"EX_Let","Variable":local("AnimGraph"),"Expression":{"Token":"EX_StructConst","Struct":{"ObjectName":"Class'PoseLink'","ObjectPath":"/Script/Engine"},"Properties":[{"Token":"EX_IntConst","Value":-1}]}}),
            json!({"Token":"EX_Return","Expression":{"Token":"EX_Cast","ConversionType":"CST_DoubleToFloat","Target":{"Token":"EX_DoubleConst","Value":0.123456789}}}),
        ]);
        let mut frame = Frame::default();
        let result = f.execute(&mut frame, &mut host(), 100).unwrap();
        assert_eq!(result.as_f64().unwrap() as f32, 0.123456789f32);
        assert_eq!(frame.locals["AnimGraph"]["fields"], json!([-1]));
    }
}
