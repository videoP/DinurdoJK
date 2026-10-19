//! Validated native material expressions. Cooked GPU shader blobs are not
//! source graphs; an importer must provide expressions or an explicit port.
use glam::Vec4;
use serde_json::Value;
#[derive(Debug, Clone)]
pub enum Op {
    Constant(Vec4),
    Time,
    Position,
    Normal,
    Uv,
    Fresnel(f32),
    Add,
    Subtract,
    Multiply,
    Divide,
    Min,
    Max,
    Power,
    Sine,
    Cosine,
    Frac,
    Saturate,
    Lerp,
    Dot,
    Normalize,
    Swizzle([usize; 4]),
}
#[derive(Debug, Clone)]
pub struct Node {
    pub op: Op,
    pub inputs: Vec<usize>,
}
#[derive(Debug, Clone)]
pub struct Program {
    pub nodes: Vec<Node>,
    pub output: usize,
    pub unlit: bool,
}
impl Program {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 256 * 1024 {
            return Err("material program exceeds limit".into());
        }
        let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if v["version"] != 1 {
            return Err("unsupported material program version".into());
        }
        let rows = v["nodes"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 256)
            .ok_or("invalid material expression count")?;
        let mut nodes = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let scalar = |key: &str, default: f32| -> Result<f32, String> {
                let n = row[key].as_f64().unwrap_or(default as f64) as f32;
                if !n.is_finite() {
                    return Err("non-finite material constant".into());
                }
                Ok(n)
            };
            let (op, count) = match row["op"].as_str().unwrap_or("") {
                "constant" => {
                    let a = row["value"]
                        .as_array()
                        .filter(|a| a.len() == 4)
                        .ok_or("invalid material constant")?;
                    let mut n = [0.; 4];
                    for i in 0..4 {
                        n[i] = a[i].as_f64().ok_or("invalid material constant")? as f32;
                    }
                    if !n.iter().all(|n| n.is_finite()) {
                        return Err("non-finite material constant".into());
                    }
                    (Op::Constant(Vec4::from_array(n)), 0)
                }
                "time" => (Op::Time, 0),
                "position" => (Op::Position, 0),
                "normal" => (Op::Normal, 0),
                "uv" => (Op::Uv, 0),
                "fresnel" => (Op::Fresnel(scalar("exponent", 5.)?.clamp(0., 64.)), 0),
                "add" => (Op::Add, 2),
                "subtract" => (Op::Subtract, 2),
                "multiply" => (Op::Multiply, 2),
                "divide" => (Op::Divide, 2),
                "min" => (Op::Min, 2),
                "max" => (Op::Max, 2),
                "power" => (Op::Power, 2),
                "sine" => (Op::Sine, 1),
                "cosine" => (Op::Cosine, 1),
                "frac" => (Op::Frac, 1),
                "saturate" => (Op::Saturate, 1),
                "lerp" => (Op::Lerp, 3),
                "dot" => (Op::Dot, 2),
                "normalize" => (Op::Normalize, 1),
                "swizzle" => {
                    let a = row["components"]
                        .as_array()
                        .filter(|a| a.len() == 4)
                        .ok_or("invalid material swizzle")?;
                    let mut channels = [0; 4];
                    for i in 0..4 {
                        channels[i] = a[i]
                            .as_u64()
                            .filter(|n| *n < 4)
                            .ok_or("invalid material swizzle")?
                            as usize;
                    }
                    (Op::Swizzle(channels), 1)
                }
                name => return Err(format!("unsupported material expression {name}")),
            };
            let inputs = row["inputs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|n| {
                    n.as_u64()
                        .filter(|n| *n < (index as u64))
                        .map(|n| n as usize)
                        .ok_or("material input must precede node".into())
                })
                .collect::<Result<Vec<_>, String>>()?;
            if inputs.len() != count {
                return Err("material expression input count mismatch".into());
            }
            nodes.push(Node { op, inputs });
        }
        let output = v["output"]
            .as_u64()
            .filter(|n| *n < (nodes.len() as u64))
            .ok_or("invalid material output")? as usize;
        Ok(Self {
            nodes,
            output,
            unlit: v["unlit"].as_bool().unwrap_or(false),
        })
    }
}
